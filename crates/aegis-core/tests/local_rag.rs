use std::fs;
use std::path::Path;

use aegis_core::contracts::SearchFilters;
use aegis_core::db::SqliteStore;
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, set_file_sensitivity, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::error::AegisError;
use aegis_core::rag::{RagAskOptions, retrieve_rag_context};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use rusqlite::params;
use tempfile::TempDir;

fn seed_file(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    rel_name: &str,
    text: &str,
) -> String {
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

    let file_path = root.join(rel_name);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, text).unwrap();

    let st = inspect_path_metadata(&file_path).unwrap().unwrap();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: None,
        path: file_path,
        size: st.size,
        mtime: st.mtime,
        ctime: st.ctime,
        blake3: Some(blake3::hash(text.as_bytes()).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    let file_id = upsert_discovered_file(store.conn(), &input).unwrap();
    insert_chunk(store.conn(), &file_id, 0, text, text.split_whitespace().count() as i64, true).unwrap();
    file_id
}

#[test]
fn rag_retrieves_grounded_context_with_citations_and_prompt() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();

    let id1 = seed_file(
        &store,
        "v-rag",
        tmp.path(),
        "atlas_notes.md",
        "The Atlas workshop gate pin is 4092 and spare keys hang near Workbench C.",
    );
    let _id2 = seed_file(
        &store,
        "v-rag",
        tmp.path(),
        "gardening.txt",
        "Tomatoes need full sun and consistent morning watering.",
    );

    let ctx = retrieve_rag_context(
        store.conn(),
        "What is the Atlas workshop gate pin?",
        None,
        &RagAskOptions::default(),
        false,
    )
    .unwrap();

    assert_eq!(ctx.excluded_sensitive_count, 0);
    assert_eq!(ctx.citations.len(), 1);
    assert_eq!(ctx.citations[0].ref_id, 1);
    assert_eq!(ctx.citations[0].file_id, id1);
    assert!(ctx.citations[0].snippet.contains("4092"));
    assert!(ctx.user_prompt.contains("[1] atlas_notes.md"));
    assert!(ctx.user_prompt.contains("4092"));
    assert!(ctx.system_prompt.contains("strictly"));
}

#[test]
fn rag_k2_excludes_quarantined_and_deleted_files() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();

    let active_id = seed_file(
        &store,
        "v-k2",
        tmp.path(),
        "active_invoice.txt",
        "Vendor Northwind invoice renewal due on October 15.",
    );
    let q_id = seed_file(
        &store,
        "v-k2",
        tmp.path(),
        "quarantined_invoice.txt",
        "Quarantined Northwind invoice duplicate code 7781.",
    );
    let del_id = seed_file(
        &store,
        "v-k2",
        tmp.path(),
        "deleted_invoice.txt",
        "Deleted Northwind invoice archive code 9982.",
    );

    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![q_id])
        .unwrap();
    let tx = store.conn_mut().transaction().unwrap();
    soft_delete_file_in_tx(&tx, &del_id).unwrap();
    tx.commit().unwrap();

    // Even if caller passes include_quarantined: Some(true), RAG enforces K2 (excluded).
    let opts = RagAskOptions {
        filters: Some(SearchFilters {
            include_quarantined: Some(true),
            ..Default::default()
        }),
        ..Default::default()
    };

    let ctx = retrieve_rag_context(store.conn(), "Northwind invoice", None, &opts, false).unwrap();
    assert_eq!(ctx.citations.len(), 1);
    assert_eq!(ctx.citations[0].file_id, active_id);
    assert!(!ctx.user_prompt.contains("7781"));
    assert!(!ctx.user_prompt.contains("9982"));
}

#[test]
fn rag_r7_excludes_sensitive_files_by_default_and_counts_them_without_leaking_text() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();

    let pub_id = seed_file(
        &store,
        "v-r7",
        tmp.path(),
        "public_roadmap.txt",
        "Beacon server rollout schedule starts in Q2.",
    );
    let sec_id = seed_file(
        &store,
        "v-r7",
        tmp.path(),
        "payroll_beacon.txt",
        "Beacon payroll confidential salary 185000 and SSN-999-00-1234.",
    );
    set_file_sensitivity(store.conn(), &sec_id, "confidential").unwrap();

    let ctx = retrieve_rag_context(
        store.conn(),
        "Beacon",
        None,
        &RagAskOptions::default(),
        false,
    )
    .unwrap();

    assert_eq!(ctx.citations.len(), 1);
    assert_eq!(ctx.citations[0].file_id, pub_id);
    assert_eq!(ctx.excluded_sensitive_count, 1);
    assert!(!ctx.user_prompt.contains("185000"));
    assert!(!ctx.user_prompt.contains("SSN-999-00-1234"));
    assert!(!ctx.user_prompt.contains("payroll_beacon.txt"));
}

#[test]
fn rag_r7_rejects_include_sensitive_when_vault_sensitive_unlock_inactive() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();

    let sec_id = seed_file(
        &store,
        "v-r7-deny",
        tmp.path(),
        "passport_vault.txt",
        "Passport number XY-998877 expires 2031.",
    );
    set_file_sensitivity(store.conn(), &sec_id, "secret").unwrap();

    let opts = RagAskOptions {
        include_sensitive: Some(true),
        ..Default::default()
    };
    let err = retrieve_rag_context(store.conn(), "Passport", None, &opts, false).unwrap_err();
    assert!(
        matches!(err, AegisError::PermissionDenied(_)),
        "expected PermissionDenied when sensitive unlock is inactive, got {err:?}"
    );
}

#[test]
fn rag_r7_allows_sensitive_files_when_explicitly_requested_and_unlocked() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();

    let sec_id = seed_file(
        &store,
        "v-r7-allow",
        tmp.path(),
        "passport_vault.txt",
        "Passport number XY-998877 expires 2031.",
    );
    set_file_sensitivity(store.conn(), &sec_id, "secret").unwrap();

    let opts = RagAskOptions {
        include_sensitive: Some(true),
        ..Default::default()
    };
    let ctx = retrieve_rag_context(store.conn(), "Passport", None, &opts, true).unwrap();

    assert_eq!(ctx.excluded_sensitive_count, 0);
    assert_eq!(ctx.citations.len(), 1);
    assert_eq!(ctx.citations[0].file_id, sec_id);
    assert!(ctx.user_prompt.contains("XY-998877"));
}
