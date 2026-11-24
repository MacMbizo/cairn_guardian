use std::fs;
use std::path::Path;

use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, rollback_batch, simulate_batch};
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, set_file_sensitivity, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::error::AegisError;
use aegis_core::folderless::{
    FolderlessRuleInput, compile_intent_deterministic, list_folderless_batches,
    list_folderless_rules, preview_folderless_rule, propose_folderless_batch,
    save_folderless_rule, toggle_folderless_rule, validate_dest_root,
};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use rusqlite::params;
use tempfile::TempDir;

const DAY_MS: i64 = 86_400_000;

fn seed_volume(store: &SqliteStore, vol_id: &str, root: &Path, role: Option<&str>) {
    let meta = inspect_volume_for_path(root).unwrap();
    let qdir = root.join(".aegis-quarantine");
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, label, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, role, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'online', ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, role = excluded.role",
            params![
                vol_id,
                root.to_string_lossy().to_string(),
                format!("Drive-{vol_id}"),
                meta.fs_type,
                i32::from(meta.has_stable_inode),
                i32::from(meta.case_sensitive),
                qdir.to_string_lossy().to_string(),
                role.unwrap_or("general"),
                aegis_core::now_ms(),
            ],
        )
        .unwrap();
}

fn seed_file(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    rel_path: &str,
    text: &str,
    mtime_ms: i64,
    atomic_unit_id: Option<&str>,
) -> String {
    let file_path = root.join(rel_path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let bytes = text.as_bytes();
    fs::write(&file_path, bytes).unwrap();
    let st = inspect_path_metadata(&file_path).unwrap().unwrap();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: atomic_unit_id.map(str::to_string),
        path: file_path,
        size: bytes.len() as i64,
        mtime: mtime_ms,
        ctime: mtime_ms,
        blake3: Some(blake3::hash(bytes).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    let fid = upsert_discovered_file(store.conn(), &input).unwrap();
    store
        .conn()
        .execute(
            "UPDATE files SET mtime = ?1, size = ?2 WHERE id = ?3",
            params![mtime_ms, bytes.len() as i64, fid],
        )
        .unwrap();
    let tok = text.split_whitespace().count() as i64;
    insert_chunk(store.conn(), &fid, 0, text, tok, true).unwrap();
    fid
}

#[test]
fn compiles_plain_language_invoice_intent_into_visible_editable_rule() {
    let tmp = TempDir::new().unwrap();
    let downloads = tmp.path().join("Downloads");
    let documents = tmp.path().join("Documents");
    fs::create_dir_all(&downloads).unwrap();
    fs::create_dir_all(&documents).unwrap();

    let store = SqliteStore::open_in_memory().unwrap();
    let roots = vec![
        downloads.to_string_lossy().to_string(),
        documents.to_string_lossy().to_string(),
    ];

    let draft = compile_intent_deterministic(
        "Route vendor invoices from Downloads to Accounting Archives, organized by fiscal quarter and company name",
        &roots,
    )
    .unwrap();

    assert_eq!(draft.compiled_by, "deterministic");
    assert_eq!(draft.rule.scope_root.as_deref(), Some(roots[0].as_str()));
    assert_eq!(draft.rule.match_doc_types, vec!["invoice"]);
    assert_eq!(draft.rule.path_template, r"{year}\{quarter}\{org}");
    assert!(draft.rule.dest_root.ends_with("Accounting Archives"));

    let saved = save_folderless_rule(store.conn(), &draft.rule).unwrap();
    assert!(saved.enabled);
    assert_eq!(list_folderless_rules(store.conn()).unwrap().len(), 1);

    let toggled = toggle_folderless_rule(store.conn(), &saved.id, false).unwrap();
    assert!(!toggled.enabled);
}

#[test]
fn content_aware_preview_extracts_quarter_org_and_amount_anchors() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = aegis_core::now_ms();
    seed_volume(&store, "v-main", tmp.path(), None);

    let dl = tmp.path().join("Downloads");
    let docs = tmp.path().join("Documents");
    fs::create_dir_all(&dl).unwrap();
    fs::create_dir_all(&docs).unwrap();
    store.add_root(tmp.path()).unwrap();

    let inv_id = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\raw_scan_04.txt",
        "Vodacom Statement & Invoice #8841\nDate: 2026-08-14\nTotal Due: R 4,250.00",
        now - 10 * DAY_MS,
        None,
    );
    let _other_id = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\holiday_notes.txt",
        "Packing list for beach trip.",
        now - 5 * DAY_MS,
        None,
    );

    let draft = compile_intent_deterministic(
        "Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name",
        &[dl.to_string_lossy().to_string(), docs.to_string_lossy().to_string()],
    )
    .unwrap();
    let report = preview_folderless_rule(store.conn(), &draft.rule, false, false, 50).unwrap();

    assert_eq!(report.total_matches, 1);
    let m = &report.matches[0];
    assert_eq!(m.file_id, inv_id);
    assert!(
        m.to_path.contains(r"Accounting Archives\2026\Q3\Vodacom\raw_scan_04.txt"),
        "unexpected destination: {}",
        m.to_path
    );
    assert!(m.extracted_anchors.iter().any(|a| a.contains("2026 Q3")));
    assert!(m.extracted_anchors.iter().any(|a| a.contains("Vodacom")));
    assert!(m.extracted_anchors.iter().any(|a| a.contains("R 4,250.00")));
}

#[test]
fn propose_simulate_approve_execute_and_rollback_roundtrip_k1() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = aegis_core::now_ms();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let f1 = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\fnb_inv.txt",
        "FNB Monthly Invoice\nDate: 2026-02-11\nAmount: R 850.00",
        now - 5 * DAY_MS,
        None,
    );

    let archive_root = tmp.path().join("Accounting Archives");
    let rule = save_folderless_rule(
        store.conn(),
        &FolderlessRuleInput {
            id: None,
            name: "FNB Invoices".into(),
            intent_text: "Route vendor invoices to Accounting Archives organized by quarter and company name".into(),
            compiled_by: None,
            enabled: Some(true),
            scope_root: Some(tmp.path().join("Downloads").to_string_lossy().to_string()),
            match_kinds: vec![],
            match_exts: vec!["txt".into()],
            match_categories: vec![],
            match_doc_types: vec!["invoice".into()],
            match_keywords: vec![],
            match_orgs: vec![],
            min_age_days: None,
            dest_root: archive_root.to_string_lossy().to_string(),
            path_template: r"{year}\{quarter}\{org}".into(),
            rename_template: Some("{date}_{org}_{stem}.{ext}".into()),
        },
    )
    .unwrap();

    let batch_id = propose_folderless_batch(
        &mut store,
        &rule.id,
        std::slice::from_ref(&f1),
        false,
        false,
    )
    .unwrap();

    let sim = simulate_batch(&store, &batch_id).unwrap();
    assert_eq!(sim.ready_count, 1);
    assert_eq!(sim.blocked_count, 0);

    approve_batch(&mut store, &batch_id, None).unwrap();
    let exec_res = execute_batch(&mut store, &batch_id).unwrap();
    assert_eq!(exec_res.executed_count, 1);

    let orig_path = tmp.path().join("Downloads\\fnb_inv.txt");
    let moved_path = archive_root.join(r"2026\Q1\FNB\2026-02-11_FNB_fnb_inv.txt");
    assert!(!orig_path.exists());
    assert!(moved_path.is_file());

    let batches = list_folderless_batches(store.conn(), 10).unwrap();
    assert_eq!(batches.len(), 1);
    assert!(batches[0].can_undo);

    let rb = rollback_batch(&mut store, &batch_id).unwrap();
    assert_eq!(rb.reverted_count, 1);
    assert!(orig_path.is_file());
    assert!(!moved_path.exists());
}

#[test]
fn protects_atomic_units_backup_drives_and_quarantined_files_k2_k4() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("Main");
    let backup_root = tmp.path().join("Backup");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&backup_root).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = aegis_core::now_ms();
    seed_volume(&store, "v-main", &main_root, Some("general"));
    seed_volume(&store, "v-backup", &backup_root, Some("backup"));
    store.add_root(&main_root).unwrap();

    store
        .conn()
        .execute(
            "INSERT INTO atomic_units (id, volume_id, root_path, unit_type, marker_file, created_at, updated_at)
             VALUES ('au-1', 'v-main', ?1, 'git_repo', '.git', ?2, ?2)",
            params![main_root.join("Repo").to_string_lossy().to_string(), now],
        )
        .unwrap();

    let _repo_file = seed_file(
        &store,
        "v-main",
        &main_root,
        "Repo\\invoice_sample.txt",
        "Invoice inside code repo must not split.",
        now - 2 * DAY_MS,
        Some("au-1"),
    );
    let fq = seed_file(
        &store,
        "v-main",
        &main_root,
        "Downloads\\quar_inv.txt",
        "Invoice quarantined",
        now - 2 * DAY_MS,
        None,
    );
    let fd = seed_file(
        &store,
        "v-main",
        &main_root,
        "Downloads\\del_inv.txt",
        "Invoice deleted",
        now - 2 * DAY_MS,
        None,
    );
    let fb = seed_file(
        &store,
        "v-backup",
        &backup_root,
        "Mirror\\inv_backup.txt",
        "Invoice on backup volume",
        now - 2 * DAY_MS,
        None,
    );

    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![fq])
        .unwrap();
    store
        .with_full_sync_tx(|tx| soft_delete_file_in_tx(tx, &fd))
        .unwrap();

    let rule_in = FolderlessRuleInput {
        id: None,
        name: "Invoices".into(),
        intent_text: "Move invoices".into(),
        compiled_by: None,
        enabled: Some(true),
        scope_root: None,
        match_kinds: vec![],
        match_exts: vec![],
        match_categories: vec![],
        match_doc_types: vec!["invoice".into()],
        match_keywords: vec![],
        match_orgs: vec![],
        min_age_days: None,
        dest_root: main_root.join("Archives").to_string_lossy().to_string(),
        path_template: "{year}".into(),
        rename_template: None,
    };
    let report = preview_folderless_rule(store.conn(), &rule_in, false, false, 50).unwrap();
    assert_eq!(report.total_matches, 0);
    assert_eq!(report.skipped_atomic_unit_count, 1);

    let saved = save_folderless_rule(store.conn(), &rule_in).unwrap();
    let backup_err = propose_folderless_batch(&mut store, &saved.id, &[fb], false, false).unwrap_err();
    assert!(matches!(backup_err, AegisError::PermissionDenied(_)));
}

#[test]
fn enforces_r7_privacy_gate_for_sensitive_documents() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = aegis_core::now_ms();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let f_sec = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\secret_invoice.txt",
        "Confidential Invoice #991\nDate: 2026-07-02\nAmount: $9,000.00",
        now - 3 * DAY_MS,
        None,
    );
    set_file_sensitivity(store.conn(), &f_sec, "confidential").unwrap();

    let rule_in = FolderlessRuleInput {
        id: None,
        name: "Invoices".into(),
        intent_text: "Route invoices to Archives".into(),
        compiled_by: None,
        enabled: Some(true),
        scope_root: None,
        match_kinds: vec![],
        match_exts: vec![],
        match_categories: vec![],
        match_doc_types: vec!["invoice".into()],
        match_keywords: vec![],
        match_orgs: vec![],
        min_age_days: None,
        dest_root: tmp.path().join("Archives").to_string_lossy().to_string(),
        path_template: "{year}".into(),
        rename_template: None,
    };
    let saved = save_folderless_rule(store.conn(), &rule_in).unwrap();

    let locked = preview_folderless_rule(store.conn(), &rule_in, false, false, 20).unwrap();
    assert_eq!(locked.total_matches, 0);
    assert_eq!(locked.excluded_sensitive_count, 1);

    let stage_err = propose_folderless_batch(
        &mut store,
        &saved.id,
        std::slice::from_ref(&f_sec),
        false,
        false,
    )
    .unwrap_err();
    assert!(matches!(stage_err, AegisError::PermissionDenied(_)));

    let unlocked = preview_folderless_rule(store.conn(), &rule_in, true, true, 20).unwrap();
    assert_eq!(unlocked.total_matches, 1);
    assert_eq!(unlocked.excluded_sensitive_count, 0);
}

#[test]
fn rejects_unsafe_destination_paths_and_handles_collisions() {
    assert!(matches!(
        validate_dest_root(r"C:\Windows\System32\Test"),
        Err(AegisError::PermissionDenied(_))
    ));
    assert!(matches!(
        validate_dest_root(r"C:\Archive\..\Escape"),
        Err(AegisError::PermissionDenied(_))
    ));

    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = aegis_core::now_ms();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let dest_dir = tmp.path().join(r"Archives\2026");
    fs::create_dir_all(&dest_dir).unwrap();
    fs::write(dest_dir.join("inv.txt"), b"existing copy").unwrap();

    let f1 = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\inv.txt",
        "Invoice #1\nDate: 2026-04-10",
        now - 2 * DAY_MS,
        None,
    );

    let rule = save_folderless_rule(
        store.conn(),
        &FolderlessRuleInput {
            id: None,
            name: "Invoices".into(),
            intent_text: "Route invoices to Archives".into(),
            compiled_by: None,
            enabled: Some(true),
            scope_root: Some(tmp.path().join("Downloads").to_string_lossy().to_string()),
            match_kinds: vec![],
            match_exts: vec![],
            match_categories: vec![],
            match_doc_types: vec!["invoice".into()],
            match_keywords: vec![],
            match_orgs: vec![],
            min_age_days: None,
            dest_root: tmp.path().join("Archives").to_string_lossy().to_string(),
            path_template: "{year}".into(),
            rename_template: None,
        },
    )
    .unwrap();

    let batch_id = propose_folderless_batch(&mut store, &rule.id, &[f1], false, false).unwrap();
    let to_path: String = store
        .conn()
        .query_row(
            "SELECT to_path FROM actions WHERE batch_id = ?1",
            params![batch_id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        to_path.ends_with("inv (1).txt"),
        "expected collision-safe filename inv (1).txt, got {to_path}"
    );
}
