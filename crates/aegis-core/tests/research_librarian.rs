use std::fs;
use std::path::Path;

use aegis_core::db::SqliteStore;
use aegis_core::db::files::{
    DiscoveredFileInput, insert_chunk, set_file_sensitivity, soft_delete_file_in_tx,
    upsert_discovered_file,
};
use aegis_core::db::vec::{PendingChunk, upsert_chunk_embeddings};
use aegis_core::error::AegisError;
use aegis_core::librarian::{
    AssociationReason, LibrarianOptions, dismiss_reference, find_related_for_file,
    find_related_for_topic, list_recent_research_docs, pin_reference, prepare_brief_context,
    undismiss_reference, unpin_reference,
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

fn seed_indexed_doc(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    rel_path: &str,
    text: &str,
    mtime_ms: i64,
) -> (String, i64) {
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
        atomic_unit_id: None,
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
    let chunk_uuid = insert_chunk(store.conn(), &fid, 0, text, tok, true).unwrap();
    let cid: i64 = store
        .conn()
        .query_row(
            "SELECT id FROM chunks WHERE chunk_uuid = ?1",
            params![chunk_uuid],
            |r| r.get(0),
        )
        .unwrap();
    (fid, cid)
}

fn seed_collection(store: &SqliteStore, col_id: &str, name: &str, vol_id: &str, root: &Path) {
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at)
             VALUES (?1, 'project', ?2, 'software', 0.95, 'confirmed', '[]', '[]', ?3)",
            params![col_id, name, now],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_locations (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
             VALUES (?1, ?2, ?3, ?4, 'home', 2, 1024, ?5, 0, 'hash', 0, 'fp-col')",
            params![
                format!("loc-{col_id}"),
                col_id,
                vol_id,
                root.to_string_lossy().to_string(),
                now
            ],
        )
        .unwrap();
}

fn seed_entity_link(
    store: &SqliteStore,
    entity_id: &str,
    entity_type: &str,
    name: &str,
    file_ids: &[&str],
) {
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT OR IGNORE INTO entities (id, entity_type, name, attrs_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, '{}', ?4, ?4)",
            params![entity_id, entity_type, name, now],
        )
        .unwrap();
    for fid in file_ids {
        store
            .conn()
            .execute(
                "INSERT INTO relationships (id, src_id, src_kind, dst_id, dst_kind, rel_type, valid_from, created_at)
                 VALUES (?1, ?2, 'file', ?3, 'entity', 'mentions', ?4, ?4)",
                params![format!("rel-{entity_id}-{fid}"), fid, entity_id, now],
            )
            .unwrap();
    }
}

#[test]
fn semantic_association_finds_conceptual_matches_with_zero_shared_keywords() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(
        &store,
        "col-alpha",
        "Project Alpha",
        "v-main",
        &tmp.path().join("Projects\\Alpha"),
    );
    seed_collection(
        &store,
        "col-papers",
        "Academic Archive",
        "v-main",
        &tmp.path().join("Archive\\Papers"),
    );

    // Completely disjoint vocabularies between alpha_spec and beta_paper, yet high vector similarity!
    let (f_src, c_src) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Projects\\Alpha\\vault_spec.md",
        "Enclave sealing isolates background telemetry daemons from biometric prompts.",
        now - DAY_MS,
    );
    let (f_paper, c_paper) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Archive\\Papers\\attestation_study.pdf",
        "Hardware TPM compartments separate automated workers against coldboot extraction.",
        now - 30 * DAY_MS,
    );
    let (f_unrelated, c_unrelated) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Archive\\Papers\\gardening_guide.txt",
        "Spring vegetable beds require compost aeration and morning irrigation.",
        now - 10 * DAY_MS,
    );

    upsert_chunk_embeddings(
        store.conn(),
        "test-embed",
        3,
        &[
            (
                PendingChunk {
                    chunk_id: c_src,
                    chunk_uuid: "u-src".into(),
                    file_id: f_src.clone(),
                    embed_text: "src".into(),
                    content_hash: [1u8; 32],
                },
                vec![1.0, 0.0, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: c_paper,
                    chunk_uuid: "u-paper".into(),
                    file_id: f_paper.clone(),
                    embed_text: "paper".into(),
                    content_hash: [2u8; 32],
                },
                vec![0.96, 0.28, 0.0], // cosine = 0.96, zero shared words
            ),
            (
                PendingChunk {
                    chunk_id: c_unrelated,
                    chunk_uuid: "u-unrel".into(),
                    file_id: f_unrelated.clone(),
                    embed_text: "unrel".into(),
                    content_hash: [3u8; 32],
                },
                vec![0.0, 0.0, 1.0],
            ),
        ],
    )
    .unwrap();

    let shelf = list_recent_research_docs(store.conn(), None, None, 20, false, false).unwrap();
    assert_eq!(shelf.total_indexed_docs, 3);
    assert_eq!(shelf.embedded_docs, 3);
    assert_eq!(shelf.docs[0].file_id, f_src);

    let report =
        find_related_for_file(store.conn(), &f_src, &LibrarianOptions::default(), false).unwrap();
    assert!(report.used_vectors);
    assert_eq!(report.references.len(), 1);
    let r = &report.references[0];
    assert_eq!(r.file_id, f_paper);
    assert!(r.semantic_only, "expected conceptual match with 0 shared terms");
    assert!(r.shared_terms.is_empty());
    assert!(r.cross_project);
    assert_eq!(r.collection_name.as_deref(), Some("Academic Archive"));
    assert_eq!(r.primary_reason, AssociationReason::ConceptualParallel);
    assert!(r.semantic_score > 0.94);
    assert_eq!(report.conceptual_count, 1);
}

#[test]
fn combines_lexical_and_approved_entity_overlap_from_knowledge_graph() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    seed_collection(
        &store,
        "col-h",
        "Helios",
        "v-main",
        &tmp.path().join("Helios"),
    );
    seed_collection(
        &store,
        "col-m",
        "Mercury",
        "v-main",
        &tmp.path().join("Mercury"),
    );

    let (f_src, _) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Helios\\charter.md",
        "Helios rollout schedule hinges on sqlite retention and failover throughput.",
        now - DAY_MS,
    );
    let (f_other, _) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Mercury\\roadmap_notes.md",
        "Mercury engineers verified failover throughput metrics for quarterly release.",
        now - 2 * DAY_MS,
    );

    seed_entity_link(
        &store,
        "ent-alice",
        "person",
        "Alice Mokoena",
        &[&f_src, &f_other],
    );
    seed_entity_link(
        &store,
        "ent-dec-1",
        "decision",
        "Adopt local-only failover cache",
        &[&f_src, &f_other],
    );

    let report =
        find_related_for_file(store.conn(), &f_src, &LibrarianOptions::default(), false).unwrap();
    assert_eq!(report.references.len(), 1);
    let r = &report.references[0];
    assert_eq!(r.file_id, f_other);
    assert!(!r.semantic_only);
    assert!(r.cross_project);
    assert_eq!(r.shared_entities.len(), 2);
    assert!(r.shared_terms.contains(&"failover".to_string()));
    assert!(r.shared_terms.contains(&"throughput".to_string()));
    assert!(r.entity_score > 0.6);
    assert!(r.lexical_score > 0.3);

    // Reading brief context works cleanly across multiple references.
    let brief = prepare_brief_context(
        store.conn(),
        Some(&f_src),
        None,
        std::slice::from_ref(&f_other),
        false,
        false,
    )
    .unwrap();
    assert_eq!(brief.citations.len(), 1);
    assert_eq!(brief.citations[0].file_id, f_other);
    assert!(brief.user_prompt.contains("[1] roadmap_notes.md"));
}

#[test]
fn topic_query_finds_related_references_across_projects() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let (f_vec, c_vec) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Specs\\cold_storage.md",
        "Air-gapped optical recovery shards preserve offline continuity.",
        now - 2 * DAY_MS,
    );
    let (f_ent, c_ent) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Meetings\\sync_log.vtt",
        "Discussed weekly milestone timeline and hardware review.",
        now - 4 * DAY_MS,
    );

    seed_entity_link(&store, "ent-acme", "organization", "Acme Robotics", &[&f_ent]);

    upsert_chunk_embeddings(
        store.conn(),
        "test-embed",
        3,
        &[
            (
                PendingChunk {
                    chunk_id: c_vec,
                    chunk_uuid: "uv".into(),
                    file_id: f_vec.clone(),
                    embed_text: "v".into(),
                    content_hash: [4u8; 32],
                },
                vec![0.98, 0.19, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: c_ent,
                    chunk_uuid: "ue".into(),
                    file_id: f_ent.clone(),
                    embed_text: "e".into(),
                    content_hash: [5u8; 32],
                },
                vec![0.0, 1.0, 0.0],
            ),
        ],
    )
    .unwrap();

    let qvec = [1.0f32, 0.0, 0.0];
    let report = find_related_for_topic(
        store.conn(),
        "What did Acme Robotics decide about continuity disaster protection?",
        Some(("test-embed", &qvec, 0.30)),
        &LibrarianOptions::default(),
        false,
    )
    .unwrap();

    let ids: Vec<&str> = report.references.iter().map(|r| r.file_id.as_str()).collect();
    assert!(ids.contains(&f_vec.as_str()), "vector hit expected");
    assert!(ids.contains(&f_ent.as_str()), "entity-name hit expected");
}

#[test]
fn pins_and_dismissals_persist_and_reorder_reading_list() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let (f_src, _) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\source.md",
        "Distributed consensus replication benchmarks for storage cluster.",
        now - DAY_MS,
    );
    let (f_high, _) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\consensus_high.md",
        "Distributed consensus replication benchmarks and latency graphs.",
        now - 2 * DAY_MS,
    );
    let (f_low, _) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "Notes\\background_low.md",
        "Short storage appendix note.",
        now - 5 * DAY_MS,
    );

    pin_reference(store.conn(), &f_src, &f_low, Some("Essential baseline appendix")).unwrap();
    let r1 =
        find_related_for_file(store.conn(), &f_src, &LibrarianOptions::default(), false).unwrap();
    assert_eq!(r1.references[0].file_id, f_low);
    assert!(r1.references[0].pinned);
    assert_eq!(r1.references[0].pin_note, "Essential baseline appendix");
    assert_eq!(r1.pinned_count, 1);

    dismiss_reference(store.conn(), &f_src, &f_high).unwrap();
    let r2 =
        find_related_for_file(store.conn(), &f_src, &LibrarianOptions::default(), false).unwrap();
    assert_eq!(r2.dismissed_count, 1);
    assert!(!r2.references.iter().any(|x| x.file_id == f_high));

    let r3 = find_related_for_file(
        store.conn(),
        &f_src,
        &LibrarianOptions {
            include_dismissed: Some(true),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert!(r3.references.iter().any(|x| x.file_id == f_high && x.dismissed));

    assert!(undismiss_reference(store.conn(), &f_src, &f_high).unwrap());
    assert!(unpin_reference(store.conn(), &f_src, &f_low).unwrap());
    let r4 =
        find_related_for_file(store.conn(), &f_src, &LibrarianOptions::default(), false).unwrap();
    assert_eq!(r4.pinned_count, 0);
    assert_eq!(r4.references[0].file_id, f_high);
}

#[test]
fn excludes_backup_drives_and_quarantined_files_k2_k4() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("MainDrive");
    let backup_root = tmp.path().join("BackupDrive");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&backup_root).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", &main_root, Some("general"));
    seed_volume(&store, "v-backup", &backup_root, Some("backup"));

    let (f_src, _) = seed_indexed_doc(
        &store,
        "v-main",
        &main_root,
        "alpha_notes.md",
        "Quantum lattice cryptography implementation checklist.",
        now - DAY_MS,
    );
    let (f_quar, _) = seed_indexed_doc(
        &store,
        "v-main",
        &main_root,
        "quarantined_copy.md",
        "Quantum lattice cryptography implementation checklist copy.",
        now - 2 * DAY_MS,
    );
    let (f_deleted, _) = seed_indexed_doc(
        &store,
        "v-main",
        &main_root,
        "deleted_copy.md",
        "Quantum lattice cryptography implementation checklist old.",
        now - 3 * DAY_MS,
    );
    let (_f_backup, _) = seed_indexed_doc(
        &store,
        "v-backup",
        &backup_root,
        "backup_mirror.md",
        "Quantum lattice cryptography implementation checklist mirror.",
        now - 2 * DAY_MS,
    );

    store
        .conn()
        .execute("UPDATE files SET state = 'quarantine' WHERE id = ?1", params![f_quar])
        .unwrap();
    store
        .with_full_sync_tx(|tx| soft_delete_file_in_tx(tx, &f_deleted))
        .unwrap();

    let shelf = list_recent_research_docs(store.conn(), None, None, 20, false, false).unwrap();
    assert_eq!(shelf.docs.len(), 1);
    assert_eq!(shelf.docs[0].file_id, f_src);

    let report =
        find_related_for_file(store.conn(), &f_src, &LibrarianOptions::default(), false).unwrap();
    assert!(report.references.is_empty());
}

#[test]
fn enforces_r7_privacy_gate_for_sensitive_source_and_candidate_files() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    let now = 2_000_000_000_000i64;
    seed_volume(&store, "v-main", tmp.path(), None);

    let (f_pub, c_pub) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "public_architecture.md",
        "Hardware security enclaves and zero-network governance design.",
        now - DAY_MS,
    );
    let (f_secret, c_secret) = seed_indexed_doc(
        &store,
        "v-main",
        tmp.path(),
        "executive_compensation_secret.md",
        "Confidential zero-network governance board payroll keys 99214.",
        now - 2 * DAY_MS,
    );
    set_file_sensitivity(store.conn(), &f_secret, "confidential").unwrap();

    upsert_chunk_embeddings(
        store.conn(),
        "test-embed",
        3,
        &[
            (
                PendingChunk {
                    chunk_id: c_pub,
                    chunk_uuid: "up".into(),
                    file_id: f_pub.clone(),
                    embed_text: "p".into(),
                    content_hash: [6u8; 32],
                },
                vec![1.0, 0.0, 0.0],
            ),
            (
                PendingChunk {
                    chunk_id: c_secret,
                    chunk_uuid: "us".into(),
                    file_id: f_secret.clone(),
                    embed_text: "s".into(),
                    content_hash: [7u8; 32],
                },
                vec![0.99, 0.14, 0.0],
            ),
        ],
    )
    .unwrap();

    // Shelf excludes sensitive file when locked & counts it.
    let shelf = list_recent_research_docs(store.conn(), None, None, 20, false, false).unwrap();
    assert_eq!(shelf.docs.len(), 1);
    assert_eq!(shelf.excluded_sensitive_count, 1);

    // Candidate secret file excluded from cross-references while locked (`R7`).
    let locked_report =
        find_related_for_file(store.conn(), &f_pub, &LibrarianOptions::default(), false).unwrap();
    assert!(locked_report.references.is_empty());
    assert_eq!(locked_report.excluded_sensitive_count, 1);

    // Asking for sensitive without unlock fails cleanly.
    let err = find_related_for_file(
        store.conn(),
        &f_pub,
        &LibrarianOptions {
            include_sensitive: Some(true),
            ..Default::default()
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(err, AegisError::PermissionDenied(_)));

    // Inspecting a confidential source file directly while locked also fails.
    let src_err =
        find_related_for_file(store.conn(), &f_secret, &LibrarianOptions::default(), false)
            .unwrap_err();
    assert!(matches!(src_err, AegisError::PermissionDenied(_)));

    // Brief synthesis also refuses confidential references while locked.
    let brief_err = prepare_brief_context(
        store.conn(),
        Some(&f_pub),
        None,
        std::slice::from_ref(&f_secret),
        false,
        false,
    )
    .unwrap_err();
    assert!(matches!(brief_err, AegisError::PermissionDenied(_)));

    // Once CredUI unlocked AND explicitly requested, confidential candidate surfaces cleanly.
    let unlocked = find_related_for_file(
        store.conn(),
        &f_pub,
        &LibrarianOptions {
            include_sensitive: Some(true),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(unlocked.references.len(), 1);
    assert_eq!(unlocked.references[0].file_id, f_secret);
    assert_eq!(unlocked.excluded_sensitive_count, 0);
}
