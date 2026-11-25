use std::fs;
use std::path::Path;

use aegis_core::db::SqliteStore;
use aegis_core::db::crypto::DbKey;
use aegis_core::db::files::SENSITIVE_MASK;
use aegis_core::error::AegisError;
use aegis_core::estate::engine::DAY_MS;
use aegis_core::estate::shamir::{combine_shards, parse_shard, split_key};
use aegis_core::estate::{
    EstateHeartbeatState, EstatePacketInput, EstatePacketKind, EstateTrusteeInput, TrusteeRole,
    build_estate_dashboard, delete_estate_packet, delete_estate_trustee,
    export_estate_continuity_kit, generate_estate_shard_set, mark_estate_shard_delivered,
    record_estate_checkin, revoke_estate_shard_set, save_estate_heartbeat_config,
    save_estate_packet, save_estate_trustee, verify_estate_shard_quorum,
};
use aegis_core::governance::posture::VaultPostureContext;
use aegis_core::scanner::os_adapter::inspect_volume_for_path;
use rusqlite::params;
use tempfile::TempDir;

fn seed_volume(store: &SqliteStore, vol_id: &str, root: &Path, role: Option<&str>) {
    let meta = inspect_volume_for_path(root).unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, label, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, role, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'online', ?8, ?9)",
            params![
                vol_id,
                root.to_string_lossy().to_string(),
                format!("Drive-{vol_id}"),
                meta.fs_type,
                i32::from(meta.has_stable_inode),
                i32::from(meta.case_sensitive),
                root.join(".aegis-quarantine").to_string_lossy().to_string(),
                role.unwrap_or("general"),
                aegis_core::now_ms(),
            ],
        )
        .unwrap();
}

#[test]
fn shamir_splits_and_reconstructs_db_key_across_any_quorum_subset() {
    let key = DbKey::generate().unwrap();
    let shards = split_key(&key, 3, 5, "estate-3of5").unwrap();
    assert_eq!(shards.len(), 5);

    let r_sub1 = combine_shards(&[
        shards[0].shard_token.to_string(),
        shards[2].shard_token.to_string(),
        shards[4].shard_token.to_string(),
    ])
    .unwrap();
    let r_sub2 = combine_shards(&[
        shards[1].shard_token.to_string(),
        shards[3].shard_token.to_string(),
        shards[4].shard_token.to_string(),
    ])
    .unwrap();
    assert_eq!(r_sub1, key);
    assert_eq!(r_sub2, key);

    assert!(combine_shards(&[
        shards[0].shard_token.to_string(),
        shards[1].shard_token.to_string(),
    ])
    .is_err());

    let mut bad = shards[0].shard_token.to_string();
    let last = bad.pop().unwrap();
    bad.push(if last == '0' { '1' } else { '0' });
    assert!(parse_shard(&bad).is_err());
}

#[test]
fn generates_shard_metadata_without_storing_raw_shards_and_verifies_quorum_r2_r7() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let key = DbKey::generate().unwrap();

    let t1 = save_estate_trustee(
        &mut store,
        &EstateTrusteeInput {
            id: None,
            name: "Alice".into(),
            relationship: "Spouse".into(),
            contact_hint: "alice@example".into(),
            role: TrusteeRole::PrimaryExecutor,
            notes: "".into(),
        },
    )
    .unwrap();

    // Locked R2/R7 refuses shard generation & verification.
    let gen_err = generate_estate_shard_set(
        &mut store,
        &key,
        2,
        3,
        &[Some(t1.id.clone()), None, None],
        false,
    )
    .unwrap_err();
    assert!(matches!(gen_err, AegisError::PermissionDenied(_)));

    let bundle = generate_estate_shard_set(
        &mut store,
        &key,
        2,
        3,
        &[Some(t1.id.clone()), None, None],
        true,
    )
    .unwrap();
    assert_eq!(bundle.shards.len(), 3);
    assert_eq!(bundle.key_fingerprint, key.fingerprint());

    // Database stores metadata ONLY — never raw CAIRN-SHARD-V1 secret strings.
    let row_count: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM estate_shard_records WHERE shard_set_id = ?1 AND revoked_at IS NULL",
            params![bundle.shard_set_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(row_count, 3);

    let tokens: Vec<String> = bundle
        .shards
        .iter()
        .take(2)
        .map(|s| s.shard_token.clone())
        .collect();
    let ver_denied = verify_estate_shard_quorum(
        &mut store,
        &tokens,
        Some(&key.fingerprint()),
        false,
    )
    .unwrap_err();
    assert!(matches!(ver_denied, AegisError::PermissionDenied(_)));

    let ver = verify_estate_shard_quorum(
        &mut store,
        &tokens,
        Some(&key.fingerprint()),
        true,
    )
    .unwrap();
    assert!(ver.valid);
    assert!(ver.matches_active_vault);
    assert_eq!(ver.key_fingerprint, key.fingerprint());

    assert!(mark_estate_shard_delivered(&mut store, &bundle.shards[0].record_id).unwrap());

    let dash = build_estate_dashboard(
        store.conn(),
        &VaultPostureContext::default(),
        true,
        aegis_core::now_ms(),
    )
    .unwrap();
    assert!(dash.shard_records[0].delivered_at.is_some());
    assert!(dash.shard_records.iter().all(|s| s.verified_at.is_some()));

    assert!(revoke_estate_shard_set(&mut store, &bundle.shard_set_id).unwrap());
    let active_after_revoke: i64 = store
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM estate_shard_records WHERE revoked_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(active_after_revoke, 0);
}

#[test]
fn trustee_and_packet_crud_masks_packet_bodies_when_locked_r2_r7() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    let tr = save_estate_trustee(
        &mut store,
        &EstateTrusteeInput {
            id: None,
            name: "Counsel Bob".into(),
            relationship: "Lawyer".into(),
            contact_hint: "office".into(),
            role: TrusteeRole::LegalCounsel,
            notes: "Safe #2".into(),
        },
    )
    .unwrap();

    let denied = save_estate_packet(
        &mut store,
        &EstatePacketInput {
            id: None,
            title: "Safe Combinations".into(),
            trustee_id: Some(tr.id.clone()),
            packet_kind: EstatePacketKind::LegalInstructions,
            body_markdown: "Master safe code 9918".into(),
            include_backup_map: true,
            include_project_inventory: false,
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(denied, AegisError::PermissionDenied(_)));

    let pkt = save_estate_packet(
        &mut store,
        &EstatePacketInput {
            id: None,
            title: "Safe Combinations".into(),
            trustee_id: Some(tr.id.clone()),
            packet_kind: EstatePacketKind::LegalInstructions,
            body_markdown: "Master safe code 9918".into(),
            include_backup_map: true,
            include_project_inventory: false,
        },
        true,
    )
    .unwrap();
    assert_eq!(pkt.trustee_name.as_deref(), Some("Counsel Bob"));

    let now = aegis_core::now_ms();
    let locked_dash =
        build_estate_dashboard(store.conn(), &VaultPostureContext::default(), false, now).unwrap();
    assert!(locked_dash.packets_masked);
    assert_eq!(locked_dash.packets.len(), 1);
    assert!(locked_dash.packets[0].masked);
    assert_eq!(locked_dash.packets[0].body_markdown, SENSITIVE_MASK);

    let unlocked_dash =
        build_estate_dashboard(store.conn(), &VaultPostureContext::default(), true, now).unwrap();
    assert!(!unlocked_dash.packets_masked);
    assert_eq!(
        unlocked_dash.packets[0].body_markdown,
        "Master safe code 9918"
    );

    assert!(delete_estate_packet(&mut store, &pkt.id, true).unwrap());
    assert!(delete_estate_trustee(&mut store, &tr.id).unwrap());
}

#[test]
fn heartbeat_transitions_through_grace_to_ready_for_release_without_deleting_files_r10() {
    let tmp = TempDir::new().unwrap();
    let protected_file = tmp.path().join("important_will.pdf");
    fs::write(&protected_file, "Legacy will instructions").unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let st0 = save_estate_heartbeat_config(&mut store, true, 30, 7).unwrap();
    assert_eq!(st0.state, EstateHeartbeatState::Active);
    let base = st0.config.last_checkin_at;

    let d_warn = build_estate_dashboard(
        store.conn(),
        &VaultPostureContext::default(),
        false,
        base + 25 * DAY_MS,
    )
    .unwrap();
    assert_eq!(d_warn.heartbeat.state, EstateHeartbeatState::Warning);

    let d_grace = build_estate_dashboard(
        store.conn(),
        &VaultPostureContext::default(),
        false,
        base + 32 * DAY_MS,
    )
    .unwrap();
    assert_eq!(d_grace.heartbeat.state, EstateHeartbeatState::GracePeriod);

    let d_release = build_estate_dashboard(
        store.conn(),
        &VaultPostureContext::default(),
        false,
        base + 40 * DAY_MS,
    )
    .unwrap();
    assert_eq!(
        d_release.heartbeat.state,
        EstateHeartbeatState::ReadyForRelease
    );
    assert!(d_release.r10_policy_summary.contains("NEVER"));
    assert!(
        protected_file.is_file(),
        "R10 must never delete or lock files on missed check-ins"
    );
}

#[test]
fn checkin_resets_overdue_heartbeat_back_to_active_and_logs_history() {
    let mut store = SqliteStore::open_in_memory().unwrap();
    save_estate_heartbeat_config(&mut store, true, 30, 14).unwrap();

    let status = record_estate_checkin(&mut store, "manual", Some("Monthly confirmation")).unwrap();
    assert_eq!(status.state, EstateHeartbeatState::Active);

    let dash = build_estate_dashboard(
        store.conn(),
        &VaultPostureContext::default(),
        false,
        aegis_core::now_ms(),
    )
    .unwrap();
    assert_eq!(dash.recent_checkins.len(), 1);
    assert_eq!(dash.recent_checkins[0].note, "Monthly confirmation");
}

#[test]
fn exports_non_destructive_continuity_kit_with_manifest_and_packets_without_raw_keys() {
    let tmp = TempDir::new().unwrap();
    let existing = tmp.path().join("existing_note.txt");
    fs::write(&existing, "Keep untouched").unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    let key = DbKey::generate().unwrap();
    let tr = save_estate_trustee(
        &mut store,
        &EstateTrusteeInput {
            id: None,
            name: "Elena".into(),
            relationship: "Spouse".into(),
            contact_hint: "vault".into(),
            role: TrusteeRole::PrimaryExecutor,
            notes: "".into(),
        },
    )
    .unwrap();
    let bundle = generate_estate_shard_set(&mut store, &key, 2, 3, &[Some(tr.id.clone())], true).unwrap();
    save_estate_packet(
        &mut store,
        &EstatePacketInput {
            id: None,
            title: "Executor Guide".into(),
            trustee_id: Some(tr.id),
            packet_kind: EstatePacketKind::Letter,
            body_markdown: "Read drives on Backup disk.".into(),
            include_backup_map: true,
            include_project_inventory: false,
        },
        true,
    )
    .unwrap();

    let denied = export_estate_continuity_kit(store.conn(), tmp.path(), false).unwrap_err();
    assert!(matches!(denied, AegisError::PermissionDenied(_)));

    let export = export_estate_continuity_kit(store.conn(), tmp.path(), true).unwrap();
    assert_eq!(export.files_written.len(), 3);
    assert!(existing.is_file(), "existing files must never be removed");

    let key_hex = key.to_hex_compact();
    for f in &export.files_written {
        let contents = fs::read_to_string(f).unwrap();
        assert!(
            !contents.contains(key_hex.as_str()),
            "continuity kit must never leak raw DbKey: {f}"
        );
        for s in &bundle.shards {
            assert!(
                !contents.contains(&s.shard_token),
                "continuity kit must never leak raw shard token: {f}"
            );
        }
    }
    let manifest = fs::read_to_string(&export.manifest_path).unwrap();
    assert!(manifest.contains("Elena"));
    assert!(manifest.contains(&key.fingerprint()));
}

#[test]
fn computes_continuity_readiness_with_backup_drives_and_r8_r10_guarantees() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-bak", tmp.path(), Some("backup"));

    let t1 = save_estate_trustee(
        &mut store,
        &EstateTrusteeInput {
            id: None,
            name: "T1".into(),
            relationship: "".into(),
            contact_hint: "".into(),
            role: TrusteeRole::PrimaryExecutor,
            notes: "".into(),
        },
    )
    .unwrap();
    let t2 = save_estate_trustee(
        &mut store,
        &EstateTrusteeInput {
            id: None,
            name: "T2".into(),
            relationship: "".into(),
            contact_hint: "".into(),
            role: TrusteeRole::CoTrustee,
            notes: "".into(),
        },
    )
    .unwrap();
    let key = DbKey::generate().unwrap();
    let bundle = generate_estate_shard_set(
        &mut store,
        &key,
        2,
        3,
        &[Some(t1.id.clone()), Some(t2.id)],
        true,
    )
    .unwrap();
    let quorum: Vec<String> = bundle.shards.iter().take(2).map(|s| s.shard_token.clone()).collect();
    verify_estate_shard_quorum(&mut store, &quorum, Some(&key.fingerprint()), true).unwrap();
    save_estate_packet(
        &mut store,
        &EstatePacketInput {
            id: None,
            title: "Plan".into(),
            trustee_id: Some(t1.id),
            packet_kind: EstatePacketKind::DriveMap,
            body_markdown: "Docs".into(),
            include_backup_map: true,
            include_project_inventory: true,
        },
        true,
    )
    .unwrap();
    save_estate_heartbeat_config(&mut store, true, 30, 14).unwrap();

    let dash = build_estate_dashboard(
        store.conn(),
        &VaultPostureContext {
            db_encrypted: true,
            recovery_confirmed: true,
            ..Default::default()
        },
        false,
        aegis_core::now_ms(),
    )
    .unwrap();
    assert_eq!(dash.readiness_score, 100.0);
    assert_eq!(dash.readiness_grade, "A");
    assert_eq!(dash.backup_drives_online, 1);
    assert!(dash.r8_policy_summary.contains("BitLocker"));
    assert!(dash.r10_policy_summary.contains("NEVER"));
}
