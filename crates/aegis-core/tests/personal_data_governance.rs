use std::fs;
use std::path::Path;

use aegis_core::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, propose_batch, rollback_batch};
use aegis_core::db::files::{
    DiscoveredFileInput, SENSITIVE_MASK, insert_chunk, upsert_discovered_file,
};
use aegis_core::drives::registry::rebase_volume_paths;
use aegis_core::error::AegisError;
use aegis_core::governance::engine::list_governance_dashboard_with_drives;
use aegis_core::governance::posture::VaultPostureContext;
use aegis_core::governance::{
    DriveEncryptionStatus, GovernanceFindingStatus, GovernanceFindingType,
    GovernancePrivacyRuleInput, GovernanceQueryOptions, GovernanceRuleAction,
    GovernanceScanOptions, apply_privacy_rule, apply_sensitivity_labels, delete_privacy_rule,
    is_path_excluded_from_ai, list_privacy_rules, propose_governance_quarantine, save_privacy_rule,
    scan_personal_data, set_finding_status, undo_sensitivity_batch,
};
use aegis_core::rag::{RagAskOptions, retrieve_rag_context};
use aegis_core::scanner::os_adapter::{inspect_path_metadata, inspect_volume_for_path};
use rusqlite::params;
use tempfile::TempDir;

fn seed_volume(store: &SqliteStore, vol_id: &str, root: &Path, role: Option<&str>) {
    let meta = inspect_volume_for_path(root).unwrap();
    let qdir = root.join(".aegis-quarantine");
    store
        .conn()
        .execute(
            "INSERT INTO volumes (id, mount_point, label, fs_type, has_stable_inode, case_sensitive, quarantine_dir, status, role, last_seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'online', ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET mount_point = excluded.mount_point, status = excluded.status, role = excluded.role, quarantine_dir = excluded.quarantine_dir",
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
) -> String {
    let file_path = root.join(rel_path);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let bytes = text.as_bytes();
    fs::write(&file_path, bytes).unwrap();
    let st = inspect_path_metadata(&file_path).unwrap().unwrap();
    let now = aegis_core::now_ms();
    let input = DiscoveredFileInput {
        volume_id: vol_id.to_string(),
        has_stable_inode: true,
        case_sensitive: false,
        file_index: st.file_index,
        nlink: st.nlink,
        atomic_unit_id: None,
        path: file_path,
        size: bytes.len() as i64,
        mtime: now,
        ctime: now,
        blake3: Some(blake3::hash(bytes).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    upsert_discovered_file(store.conn(), &input).unwrap()
}

fn sample_aws_token() -> String {
    ["AKIA", "Z9876543210ABCDE"].concat()
}

fn sample_github_token() -> String {
    ["ghp_", "9xK2mP7qL4nV8wR1tY5uJ3hF6gD0sA"].concat()
}

fn sample_card_number() -> String {
    ["4532", "-0151", "-1283", "-0366"].concat()
}

fn sample_pem_header() -> String {
    ["-----BEGIN ", "RSA PRIVATE KEY-----"].concat()
}

#[test]
fn detects_pii_and_secrets_with_redacted_snippets_and_deduplicates_fingerprints() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let aws_tok = sample_aws_token();
    let card = sample_card_number();
    let pem = sample_pem_header();

    let env_a = format!("AWS_ACCESS_KEY_ID={aws_tok}\n");
    let env_b = format!("backup_aws_key={aws_tok}\nBilling: {card}\n{pem}\n");

    let _f1 = seed_file(&store, "v-main", tmp.path(), "Downloads\\.env", &env_a);
    let _f2 = seed_file(&store, "v-main", tmp.path(), "Notes\\deploy.txt", &env_b);

    let report =
        scan_personal_data(store.conn(), &GovernanceScanOptions::default(), false).unwrap();
    assert_eq!(report.files_inspected, 2);
    assert!(report.findings_discovered >= 4, "report: {report:#?}");

    let dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();

    let aws_findings: Vec<_> = dash
        .findings
        .iter()
        .filter(|f| f.finding_type == GovernanceFindingType::CloudApiKey)
        .collect();
    assert_eq!(
        aws_findings.len(),
        1,
        "identical AWS token across two files must deduplicate to one finding: {aws_findings:#?}"
    );
    assert_eq!(aws_findings[0].locations.len(), 2);

    assert!(
        dash.findings
            .iter()
            .any(|f| f.finding_type == GovernanceFindingType::PrivateKeyBlock)
    );
    assert!(
        dash.findings
            .iter()
            .any(|f| f.finding_type == GovernanceFindingType::PaymentCard)
    );
    assert!(
        dash.findings
            .iter()
            .any(|f| f.finding_type == GovernanceFindingType::CredentialFile)
    );

    // Raw secrets must NEVER appear in DTOs or stored DB rows.
    for finding in &dash.findings {
        assert!(!finding.summary.contains(&aws_tok));
        assert!(!finding.summary.contains("4532-0151"));
        for loc in &finding.locations {
            assert!(!loc.redacted_snippet.contains(&aws_tok));
            assert!(!loc.redacted_snippet.contains("4532-0151"));
        }
    }
    let raw_snips: Vec<String> = {
        let mut st = store
            .conn()
            .prepare("SELECT redacted_snippet FROM security_finding_locations")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(!raw_snips.is_empty());
    for snip in raw_snips {
        assert!(!snip.contains(&aws_tok));
        assert!(!snip.contains("4532-0151"));
    }
}

#[test]
fn protects_findings_with_r7_labels_and_supports_single_click_undo_k1() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let gh_tok = sample_github_token();
    let fid = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\tokens.txt",
        &format!("GITHUB_TOKEN={gh_tok}\n"),
    );

    let before_sens: String = store
        .conn()
        .query_row(
            "SELECT COALESCE(sensitivity, 'normal') FROM files WHERE id = ?1",
            params![fid],
            |r| r.get(0),
        )
        .unwrap();

    scan_personal_data(store.conn(), &GovernanceScanOptions::default(), false).unwrap();
    let dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert_eq!(dash.findings.len(), 1);
    let finding_id = dash.findings[0].id.clone();

    let batch = apply_sensitivity_labels(
        &mut store,
        std::slice::from_ref(&fid),
        "secret",
        std::slice::from_ref(&finding_id),
        "Protect exposed token",
    )
    .unwrap();
    assert!(batch.can_undo);
    assert_eq!(batch.file_count, 1);
    assert_eq!(batch.target_sensitivity, "secret");

    let after_sens: String = store
        .conn()
        .query_row(
            "SELECT sensitivity FROM files WHERE id = ?1",
            params![fid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_sens, "secret");

    let vaulted_status: String = store
        .conn()
        .query_row(
            "SELECT status FROM security_findings WHERE id = ?1",
            params![finding_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(vaulted_status, "vaulted");

    // K1 single-click undo restores prior file sensitivity and reopens finding.
    let undone = undo_sensitivity_batch(&mut store, &batch.id).unwrap();
    assert!(!undone.can_undo);
    assert_eq!(undone.status, "undone");

    let restored_sens: String = store
        .conn()
        .query_row(
            "SELECT COALESCE(sensitivity, 'normal') FROM files WHERE id = ?1",
            params![fid],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(restored_sens, before_sens);

    let reopened_status: String = store
        .conn()
        .query_row(
            "SELECT status FROM security_findings WHERE id = ?1",
            params![finding_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reopened_status, "open");
}

#[test]
fn preserves_triage_decisions_across_rescans_and_gates_sensitive_content_r7() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let f_fp = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Docs\\fp.txt",
        &format!("CARD={}\n", sample_card_number()),
    );
    let f_sens = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Private\\keys.txt",
        &format!("GITHUB_TOKEN={}\n", sample_github_token()),
    );

    scan_personal_data(store.conn(), &GovernanceScanOptions::default(), false).unwrap();
    let initial = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert_eq!(initial.findings.len(), 2);

    let fp_finding = initial
        .findings
        .iter()
        .find(|f| f.locations.iter().any(|l| l.file_id == f_fp))
        .unwrap()
        .id
        .clone();
    set_finding_status(&mut store, &fp_finding, GovernanceFindingStatus::FalsePositive).unwrap();

    // Label second file confidential (leaving its finding or protecting before rescan).
    apply_sensitivity_labels(
        &mut store,
        std::slice::from_ref(&f_sens),
        "confidential",
        &[],
        "Label confidential",
    )
    .unwrap();

    // Locked rescan must preserve false_positive status and exclude sensitive file from reading.
    let rescan = scan_personal_data(
        store.conn(),
        &GovernanceScanOptions {
            force_rescan: true,
            include_sensitive: false,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    assert_eq!(rescan.excluded_sensitive_count, 1);

    // Explicitly requesting include_sensitive without CredUI unlock must fail with PermissionDenied (R7).
    let scan_denied = scan_personal_data(
        store.conn(),
        &GovernanceScanOptions {
            include_sensitive: true,
            ..Default::default()
        },
        false,
    )
    .unwrap_err();
    assert!(matches!(scan_denied, AegisError::PermissionDenied(_)));

    let view_denied = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions {
            include_sensitive: true,
            ..Default::default()
        },
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap_err();
    assert!(matches!(view_denied, AegisError::PermissionDenied(_)));

    // Locked dashboard masks snippets of confidential/secret files with SENSITIVE_MASK.
    let locked_dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert!(locked_dash.excluded_sensitive_count >= 1);
    let fp_after = locked_dash
        .findings
        .iter()
        .find(|f| f.id == fp_finding)
        .unwrap();
    assert_eq!(fp_after.status, GovernanceFindingStatus::FalsePositive);

    let sens_loc_locked = locked_dash
        .findings
        .iter()
        .flat_map(|f| &f.locations)
        .find(|l| l.file_id == f_sens)
        .unwrap();
    assert_eq!(sens_loc_locked.redacted_snippet, SENSITIVE_MASK);

    // Unlocked dashboard reveals the redacted snippet (still never raw secret!).
    let unlocked_dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions {
            include_sensitive: true,
            ..Default::default()
        },
        &VaultPostureContext {
            sensitive_unlocked: true,
            ..Default::default()
        },
        true,
        vec![],
    )
    .unwrap();
    assert_eq!(unlocked_dash.excluded_sensitive_count, 0);
    let sens_loc_unlocked = unlocked_dash
        .findings
        .iter()
        .flat_map(|f| &f.locations)
        .find(|l| l.file_id == f_sens)
        .unwrap();
    assert_ne!(sens_loc_unlocked.redacted_snippet, SENSITIVE_MASK);
}

#[test]
fn stages_exposed_secret_file_for_reversible_30_day_quarantine_k1_k2_r8() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let fid = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\leaked.txt",
        &format!("AWS={}\n", sample_aws_token()),
    );
    let orig_path = tmp.path().join("Downloads\\leaked.txt");
    assert!(orig_path.is_file());

    scan_personal_data(store.conn(), &GovernanceScanOptions::default(), false).unwrap();

    let batch_id =
        propose_governance_quarantine(&mut store, std::slice::from_ref(&fid), false, false).unwrap();
    approve_batch(&mut store, &batch_id, None).unwrap();
    execute_batch(&mut store, &batch_id).unwrap();

    assert!(
        !orig_path.exists(),
        "file must be moved to local 30-day quarantine dir"
    );

    let dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert_eq!(dash.posture.quarantined_items_count, 1);

    let rb = rollback_batch(&mut store, &batch_id).unwrap();
    assert_eq!(rb.reverted_count, 1);
    assert!(orig_path.is_file(), "rollback must restore quarantined file");
}

#[test]
fn excludes_backup_drives_and_quarantined_files_from_scanning_k2_k4() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("main");
    let backup_root = tmp.path().join("backup");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&backup_root).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", &main_root, None);
    seed_volume(&store, "v-bak", &backup_root, Some("backup"));
    store.add_root(&main_root).unwrap();

    // Backup-drive file must never be scanned (K4).
    let bak_fid = seed_file(
        &store,
        "v-bak",
        &backup_root,
        "bak_secret.txt",
        &format!("BACKUP_AWS={}\n", sample_aws_token()),
    );

    // Quarantined file on main drive must also be excluded from scanning (K2).
    let q_fid = seed_file(
        &store,
        "v-main",
        &main_root,
        "old_key.txt",
        &format!("GITHUB_TOKEN={}\n", sample_github_token()),
    );
    let q_path = main_root.join("old_key.txt");
    let q_batch = propose_batch(
        &mut store,
        &RefilePlan {
            origin: "test".into(),
            actions: vec![ProposedActionInput {
                agent: Some("Housekeeper".into()),
                action_type: ActionType::Quarantine,
                target_kind: TargetKind::File,
                file_id: Some(q_fid.clone()),
                atomic_unit_id: None,
                from_path: q_path.to_string_lossy().to_string(),
                to_path: None,
                reason: "Quarantine before governance scan".into(),
                confidence: 0.99,
                evidence_json: "{}".into(),
            }],
        },
    )
    .unwrap();
    approve_batch(&mut store, &q_batch, None).unwrap();
    execute_batch(&mut store, &q_batch).unwrap();

    let report =
        scan_personal_data(store.conn(), &GovernanceScanOptions::default(), false).unwrap();
    assert_eq!(report.files_inspected, 0);
    assert_eq!(report.findings_discovered, 0);

    let dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert!(dash.findings.is_empty());
    let scanned_ids: Vec<String> = {
        let mut st = store
            .conn()
            .prepare("SELECT file_id FROM governance_file_scans")
            .unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };
    assert!(!scanned_ids.contains(&bak_fid));
    assert!(!scanned_ids.contains(&q_fid));
}

#[test]
fn privacy_rules_match_patterns_apply_labels_with_undo_and_rebase_on_reletter() {
    let tmp = TempDir::new().unwrap();
    let old_mount = tmp.path().join("mount_a");
    let new_mount = tmp.path().join("mount_b");
    fs::create_dir_all(&old_mount).unwrap();
    fs::create_dir_all(&new_mount).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", &old_mount, None);
    store.add_root(&old_mount).unwrap();

    let env_id = seed_file(
        &store,
        "v-main",
        &old_mount,
        "Projects\\.env.production",
        "DB_PASS=not_empty\n",
    );
    let readme_id = seed_file(&store, "v-main", &old_mount, "Projects\\README.md", "Public docs\n");

    let sub_prefix = old_mount.join("Projects").to_string_lossy().to_string();
    let auto_rule = save_privacy_rule(
        store.conn(),
        &GovernancePrivacyRuleInput {
            id: None,
            name: "Protect production env files".into(),
            path_prefix: Some(sub_prefix),
            filename_pattern: Some("*.env*".into()),
            rule_action: GovernanceRuleAction::AutoSecret,
            enabled: Some(true),
        },
    )
    .unwrap();

    let ai_rule = save_privacy_rule(
        store.conn(),
        &GovernancePrivacyRuleInput {
            id: None,
            name: "Exclude Projects from AI".into(),
            path_prefix: Some(old_mount.join("Projects").to_string_lossy().to_string()),
            filename_pattern: None,
            rule_action: GovernanceRuleAction::ExcludeFromAi,
            enabled: Some(true),
        },
    )
    .unwrap();

    let before_env_sens: String = store
        .conn()
        .query_row(
            "SELECT COALESCE(sensitivity, 'normal') FROM files WHERE id = ?1",
            params![env_id],
            |r| r.get(0),
        )
        .unwrap();

    let batch = apply_privacy_rule(&mut store, &auto_rule.id).unwrap();
    assert_eq!(batch.file_count, 1);
    assert_eq!(batch.target_sensitivity, "secret");

    let after_env_sens: String = store
        .conn()
        .query_row(
            "SELECT sensitivity FROM files WHERE id = ?1",
            params![env_id],
            |r| r.get(0),
        )
        .unwrap();
    let readme_sens: String = store
        .conn()
        .query_row(
            "SELECT COALESCE(sensitivity, 'normal') FROM files WHERE id = ?1",
            params![readme_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after_env_sens, "secret");
    assert_ne!(readme_sens, "secret");

    undo_sensitivity_batch(&mut store, &batch.id).unwrap();
    let restored_env_sens: String = store
        .conn()
        .query_row(
            "SELECT COALESCE(sensitivity, 'normal') FROM files WHERE id = ?1",
            params![env_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(restored_env_sens, before_env_sens);

    let ai_apply_err = apply_privacy_rule(&mut store, &ai_rule.id).unwrap_err();
    assert!(matches!(ai_apply_err, AegisError::InvalidInput(_)));

    // Simulate drive re-lettering / remount via rebase_volume_paths.
    let old_str = old_mount.to_string_lossy().to_string();
    let new_str = new_mount.to_string_lossy().to_string();
    rebase_volume_paths(store.conn(), "v-main", &old_str, &new_str, false).unwrap();

    let rules_after = list_privacy_rules(store.conn()).unwrap();
    let rebased_ai = rules_after.iter().find(|r| r.id == ai_rule.id).unwrap();
    let expected_prefix = new_mount.join("Projects").to_string_lossy().to_string();
    assert_eq!(
        rebased_ai.path_prefix.as_deref(),
        Some(expected_prefix.as_str())
    );

    assert!(delete_privacy_rule(store.conn(), &auto_rule.id).unwrap());
    assert!(!delete_privacy_rule(store.conn(), &auto_rule.id).unwrap());
}

#[test]
fn builds_honest_privacy_posture_report_r8_r9_r11_and_rag_ai_exclusion() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let private_text = "Northwind confidential personal tax budget ledger.";
    let public_text = "Northwind public architecture notes for workspace planning.";

    let f_private = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Private\\budget_notes.md",
        private_text,
    );
    let f_public = seed_file(
        &store,
        "v-main",
        tmp.path(),
        "Work\\public_notes.md",
        public_text,
    );
    insert_chunk(store.conn(), &f_private, 0, private_text, 6, true).unwrap();
    insert_chunk(store.conn(), &f_public, 0, public_text, 7, true).unwrap();

    let private_dir = tmp.path().join("Private").to_string_lossy().to_string();
    save_privacy_rule(
        store.conn(),
        &GovernancePrivacyRuleInput {
            id: None,
            name: "Exclude Private from AI".into(),
            path_prefix: Some(private_dir),
            filename_pattern: None,
            rule_action: GovernanceRuleAction::ExcludeFromAi,
            enabled: Some(true),
        },
    )
    .unwrap();

    let priv_path = tmp
        .path()
        .join("Private\\budget_notes.md")
        .to_string_lossy()
        .to_string();
    let pub_path = tmp
        .path()
        .join("Work\\public_notes.md")
        .to_string_lossy()
        .to_string();
    assert!(is_path_excluded_from_ai(store.conn(), &priv_path));
    assert!(!is_path_excluded_from_ai(store.conn(), &pub_path));

    let rag_ctx = retrieve_rag_context(
        store.conn(),
        "Northwind",
        None,
        &RagAskOptions::default(),
        false,
    )
    .unwrap();
    assert_eq!(rag_ctx.citations.len(), 1);
    assert_eq!(rag_ctx.citations[0].file_id, f_public);
    assert!(!rag_ctx.user_prompt.contains("personal tax budget ledger"));
    assert!(!rag_ctx.system_prompt.contains("personal tax budget ledger"));

    let dash = list_governance_dashboard_with_drives(
        store.conn(),
        &GovernanceQueryOptions::default(),
        &VaultPostureContext {
            db_encrypted: true,
            cipher_label: "SQLCipher 4.x".into(),
            dpapi_sealed: true,
            recovery_confirmed: true,
            sensitive_unlocked: false,
            sensitive_expires_at: None,
        },
        false,
        vec![DriveEncryptionStatus {
            mount_point: tmp.path().to_string_lossy().to_string(),
            label: "Workspace".into(),
            protection_status: "unknown_requires_admin".into(),
            encryption_method: "Non-admin user check (R1)".into(),
            recommended_action: "Check Windows Settings -> Device encryption".into(),
        }],
    )
    .unwrap();

    let p = &dash.posture;
    assert!(p.zero_network_status.contains("r9_forbids_networking_crates_in_engine_and_models"));
    assert!(!p.zero_network_status.to_ascii_lowercase().contains("mathematical"));
    assert!(p.shredding_policy.contains("Multi-pass shredding disabled (R8)"));
    assert!(p.shredding_policy.contains("BitLocker"));
    assert!(p.firewall_command.contains("New-NetFirewallRule"));

    let audit = |id: &str| {
        p.opt_in_audits
            .iter()
            .find(|a| a.id == id)
            .unwrap_or_else(|| panic!("missing audit {id}"))
    };
    assert_eq!(audit("screen_capture").state, "permanently_disabled");
    assert_eq!(audit("keystroke_logging").state, "permanently_disabled");
    assert_eq!(audit("browser_history").state, "off");
    assert_eq!(audit("audio_transcripts").state, "off");
    assert_eq!(audit("backup_isolation").state, "enforced");
}
