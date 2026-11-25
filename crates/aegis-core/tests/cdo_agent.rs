use std::fs;
use std::path::Path;

use aegis_core::cdo::{
    CdoBottleneckKind, CdoExecutiveBriefing, CdoProposalAction, CdoRecommendation,
    CdoRecommendationStatus, CdoScanOptions, build_cdo_dashboard_with_drives, list_cdo_briefings,
    prepare_cdo_briefing_prompt, ratify_cdo_recommendation, save_cdo_briefing_snapshot,
    set_cdo_recommendation_status, undo_cdo_ratification,
};
use aegis_core::cdo::patterns::DAY_MS;
use aegis_core::contracts::{ActionType, ProposedActionInput, RefilePlan, TargetKind};
use aegis_core::db::SqliteStore;
use aegis_core::db::actions::{approve_batch, execute_batch, propose_batch};
use aegis_core::db::files::{DiscoveredFileInput, set_file_sensitivity, upsert_discovered_file};
use aegis_core::error::AegisError;
use aegis_core::folderless::list_folderless_rules;
use aegis_core::governance::posture::VaultPostureContext;
use aegis_core::governance::{
    GovernanceScanOptions, list_privacy_rules, scan_personal_data,
};
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

fn seed_file_with_mtime(
    store: &SqliteStore,
    vol_id: &str,
    root: &Path,
    rel_path: &str,
    text: &str,
    mtime: i64,
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
        atomic_unit_id: None,
        path: file_path,
        size: bytes.len() as i64,
        mtime,
        ctime: mtime,
        blake3: Some(blake3::hash(bytes).as_bytes().to_vec()),
        cloud_state: st.cloud_state,
    };
    upsert_discovered_file(store.conn(), &input).unwrap()
}

fn seed_project_collection(
    store: &SqliteStore,
    col_id: &str,
    name: &str,
    vol_id: &str,
    root_path: &Path,
) {
    let now = aegis_core::now_ms();
    store
        .conn()
        .execute(
            "INSERT INTO collections (id, kind, name, project_kind, confidence, state, evidence_json, sub_projects_json, computed_at)
             VALUES (?1, 'project', ?2, 'code', 0.95, 'confirmed', '[]', '[]', ?3)",
            params![col_id, name, now],
        )
        .unwrap();
    store
        .conn()
        .execute(
            "INSERT INTO collection_locations
             (id, collection_id, volume_id, root_path, relationship, files, bytes, newest_mtime, shared_files, compared_by, is_repo, fingerprint)
             VALUES (?1, ?2, ?3, ?4, 'primary', 0, 0, ?5, 0, 'root', 1, ?6)",
            params![
                aegis_core::new_id(),
                col_id,
                vol_id,
                root_path.to_string_lossy().to_string(),
                now,
                format!("fp-{col_id}")
            ],
        )
        .unwrap();
}

#[test]
fn synthesizes_project_pulse_and_detects_doc_lag_behind_code_5_2() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let prj_root = tmp.path().join("Projects\\alpha-service");
    seed_project_collection(&store, "col-alpha", "alpha-service", "v-main", &prj_root);

    let now = aegis_core::now_ms();
    let code_time = now - DAY_MS;
    let old_doc_time = now - 16 * DAY_MS;

    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\alpha-service\\src\\main.rs", "fn main() {}", code_time);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\alpha-service\\src\\lib.rs", "pub fn x() {}", code_time);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\alpha-service\\src\\api.rs", "pub fn y() {}", code_time);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\alpha-service\\README.md", "# Alpha\nOld docs", old_doc_time);

    let dash = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();

    assert_eq!(dash.project_pulses.len(), 1);
    let pulse = &dash.project_pulses[0];
    assert_eq!(pulse.name, "alpha-service");
    assert_eq!(pulse.code_files, 3);
    assert_eq!(pulse.doc_files, 1);
    assert_eq!(pulse.health_flag, "doc_lagging");
    assert!(pulse.doc_lag_days.unwrap_or(0) >= 14);

    assert!(
        dash.bottlenecks
            .iter()
            .any(|b| b.kind == CdoBottleneckKind::DocLagBehindCode)
    );
    assert!(
        dash.recommendations
            .iter()
            .any(|r| r.rec_key == "cdo:doc-lag:review")
    );
}

#[test]
fn diagnoses_manual_invoice_filing_friction_and_ratifies_folderless_rule_with_undo_k1_r6() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let now = aegis_core::now_ms();
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\acme_invoice_2026.pdf", "%PDF-1.4\nInvoice", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\cloud_invoice_feb.pdf", "%PDF-1.4\nInvoice", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Desktop\\tax_receipt_01.pdf", "%PDF-1.4\nReceipt", now);

    let dash = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();

    assert_eq!(dash.velocity.loose_invoice_docs, 3);
    assert!(
        dash.bottlenecks
            .iter()
            .any(|b| b.kind == CdoBottleneckKind::ManualFilingFriction)
    );
    let rec = dash
        .recommendations
        .iter()
        .find(|r| r.rec_key == "cdo:folderless:invoice-automation")
        .expect("expected invoice automation proposal")
        .clone();
    assert!(rec.can_ratify_directly);
    assert!(matches!(rec.proposal, CdoProposalAction::CreateFolderlessRule { .. }));

    let rat = ratify_cdo_recommendation(&mut store, &rec).unwrap();
    assert!(rat.can_undo);
    assert_eq!(rat.action_kind, "create_folderless_rule");

    let rules_after = list_folderless_rules(store.conn()).unwrap();
    assert_eq!(rules_after.len(), 1);
    assert_eq!(rules_after[0].id, rat.target_ref);

    let dash_after = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert_eq!(dash_after.velocity.active_folderless_rules, 1);
    let rec_after = dash_after
        .recommendations
        .iter()
        .find(|r| r.rec_key == rec.rec_key)
        .unwrap();
    assert_eq!(rec_after.status, CdoRecommendationStatus::Ratified);

    let undone = undo_cdo_ratification(&mut store, &rat.id).unwrap();
    assert!(!undone.can_undo);
    assert_eq!(undone.status, "undone");
    assert!(list_folderless_rules(store.conn()).unwrap().is_empty());

    let dash_undone = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    let rec_reopened = dash_undone
        .recommendations
        .iter()
        .find(|r| r.rec_key == rec.rec_key)
        .unwrap();
    assert_eq!(rec_reopened.status, CdoRecommendationStatus::Open);
}

#[test]
fn diagnoses_exposed_secrets_and_ratifies_privacy_rule_with_undo_k1_r7() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let now = aegis_core::now_ms();
    let aws_tok = ["AKIA", "Z9876543210ABCDE"].concat();
    seed_file_with_mtime(
        &store,
        "v-main",
        tmp.path(),
        "Downloads\\.env",
        &format!("AWS_ACCESS_KEY_ID={aws_tok}\n"),
        now,
    );
    scan_personal_data(store.conn(), &GovernanceScanOptions::default(), false).unwrap();

    let dash = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert!(
        dash.bottlenecks
            .iter()
            .any(|b| b.kind == CdoBottleneckKind::SecurityExposureDebt)
    );
    let rec = dash
        .recommendations
        .iter()
        .find(|r| matches!(r.proposal, CdoProposalAction::CreatePrivacyRule { .. }))
        .expect("expected CreatePrivacyRule proposal")
        .clone();

    let rat = ratify_cdo_recommendation(&mut store, &rec).unwrap();
    assert_eq!(rat.action_kind, "create_privacy_rule");
    let rules = list_privacy_rules(store.conn()).unwrap();
    assert_eq!(rules.len(), 1);

    let undone = undo_cdo_ratification(&mut store, &rat.id).unwrap();
    assert!(!undone.can_undo);
    assert!(list_privacy_rules(store.conn()).unwrap().is_empty());
}

#[test]
fn ratifies_schedule_setting_adjustment_and_restores_previous_value_on_undo_k1() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let now = aegis_core::now_ms();
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\invoice1.pdf", "inv", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\invoice2.pdf", "inv", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\invoice3.pdf", "inv", now);

    // 1. Setting initially unset -> ratify sets "4", undo removes row cleanly.
    let dash = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    let sched_rec = dash
        .recommendations
        .iter()
        .find(|r| r.rec_key == "cdo:schedule:weekly-digest-friday")
        .expect("expected weekly digest schedule recommendation")
        .clone();

    let rat1 = ratify_cdo_recommendation(&mut store, &sched_rec).unwrap();
    assert_eq!(
        store.get_setting("engine.note_day", "global").unwrap().as_deref(),
        Some("4")
    );
    undo_cdo_ratification(&mut store, &rat1.id).unwrap();
    assert_eq!(store.get_setting("engine.note_day", "global").unwrap(), None);

    // 2. Setting had prior value "6" -> ratify updates to "4", undo restores "6".
    store.set_setting("engine.note_day", "global", "6").unwrap();
    let rat2 = ratify_cdo_recommendation(&mut store, &sched_rec).unwrap();
    assert_eq!(
        store.get_setting("engine.note_day", "global").unwrap().as_deref(),
        Some("4")
    );
    undo_cdo_ratification(&mut store, &rat2.id).unwrap();
    assert_eq!(
        store.get_setting("engine.note_day", "global").unwrap().as_deref(),
        Some("6")
    );

    // 3. Navigation recommendations refuse direct ratification (`InvalidInput`).
    let nav_rec = CdoRecommendation {
        rec_key: "cdo:nav".into(),
        title: "Open".into(),
        rationale: "".into(),
        bottleneck_kind: CdoBottleneckKind::PendingReviewQueue,
        severity: aegis_core::cdo::CdoSeverity::Low,
        status: CdoRecommendationStatus::Open,
        owner_note: String::new(),
        can_ratify_directly: false,
        proposal: CdoProposalAction::NavigateToScreen {
            screen: aegis_core::engine::Screen::Drives,
            label: "Open Drives".into(),
        },
    };
    let nav_err = ratify_cdo_recommendation(&mut store, &nav_rec).unwrap_err();
    assert!(matches!(nav_err, AegisError::InvalidInput(_)));
}

#[test]
fn persists_triage_status_snooze_and_dismiss_across_rescans() {
    let tmp = TempDir::new().unwrap();
    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let now = aegis_core::now_ms();
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\invoice1.pdf", "inv", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\invoice2.pdf", "inv", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Downloads\\invoice3.pdf", "inv", now);

    set_cdo_recommendation_status(
        &mut store,
        "cdo:folderless:invoice-automation",
        CdoRecommendationStatus::Snoozed,
        Some("Review Friday"),
    )
    .unwrap();

    let dash_snoozed = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    let snoozed_rec = dash_snoozed
        .recommendations
        .iter()
        .find(|r| r.rec_key == "cdo:folderless:invoice-automation")
        .unwrap();
    assert_eq!(snoozed_rec.status, CdoRecommendationStatus::Snoozed);
    assert_eq!(snoozed_rec.owner_note, "Review Friday");

    set_cdo_recommendation_status(
        &mut store,
        "cdo:folderless:invoice-automation",
        CdoRecommendationStatus::Dismissed,
        None,
    )
    .unwrap();

    let dash_hidden = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert!(
        !dash_hidden
            .recommendations
            .iter()
            .any(|r| r.rec_key == "cdo:folderless:invoice-automation")
    );

    let dash_shown = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions {
            include_dismissed: true,
            ..Default::default()
        },
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert!(
        dash_shown
            .recommendations
            .iter()
            .any(|r| r.rec_key == "cdo:folderless:invoice-automation"
                && r.status == CdoRecommendationStatus::Dismissed)
    );
}

#[test]
fn enforces_r7_sensitive_gate_k2_quarantine_and_k4_backup_exclusions() {
    let tmp = TempDir::new().unwrap();
    let main_root = tmp.path().join("main");
    let bak_root = tmp.path().join("backup");
    fs::create_dir_all(&main_root).unwrap();
    fs::create_dir_all(&bak_root).unwrap();

    let mut store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", &main_root, None);
    seed_volume(&store, "v-bak", &bak_root, Some("backup"));
    store.add_root(&main_root).unwrap();

    let now = aegis_core::now_ms();
    // 1. Backup drive invoice must never count (`K4`).
    seed_file_with_mtime(&store, "v-bak", &bak_root, "Downloads\\bak_invoice.pdf", "bak", now);

    // 2. Quarantined invoice on main drive must never count (`K2`).
    let q_fid = seed_file_with_mtime(
        &store,
        "v-main",
        &main_root,
        "Downloads\\q_invoice.pdf",
        "quarantined",
        now,
    );
    let q_path = main_root.join("Downloads\\q_invoice.pdf");
    let q_batch = propose_batch(
        &mut store,
        &RefilePlan {
            origin: "test".into(),
            actions: vec![ProposedActionInput {
                agent: Some("Housekeeper".into()),
                action_type: ActionType::Quarantine,
                target_kind: TargetKind::File,
                file_id: Some(q_fid),
                atomic_unit_id: None,
                from_path: q_path.to_string_lossy().to_string(),
                to_path: None,
                reason: "Quarantine before CDO check".into(),
                confidence: 0.99,
                evidence_json: "{}".into(),
            }],
        },
    )
    .unwrap();
    approve_batch(&mut store, &q_batch, None).unwrap();
    execute_batch(&mut store, &q_batch).unwrap();

    // 3. Confidential invoice in Downloads is masked when locked (`R7`).
    let sec_fid = seed_file_with_mtime(
        &store,
        "v-main",
        &main_root,
        "Downloads\\secret_tax_invoice.pdf",
        "secret tax",
        now,
    );
    set_file_sensitivity(store.conn(), &sec_fid, "secret").unwrap();

    let locked = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert_eq!(locked.velocity.loose_invoice_docs, 0);
    assert!(locked.excluded_sensitive_count >= 1);

    let denied_dash = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions {
            include_sensitive: true,
            ..Default::default()
        },
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap_err();
    assert!(matches!(denied_dash, AegisError::PermissionDenied(_)));

    let denied_brief = prepare_cdo_briefing_prompt(
        store.conn(),
        30,
        true,
        &VaultPostureContext::default(),
        false,
        false,
    )
    .unwrap_err();
    assert!(matches!(denied_brief, AegisError::PermissionDenied(_)));

    // When owner unlocks sensitive views (`reveal_sensitive = true`), secret invoice counts cleanly
    // while backup and quarantined files stay excluded.
    let unlocked = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions {
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
    assert_eq!(unlocked.velocity.loose_invoice_docs, 1);
}

#[test]
fn prepares_grounded_executive_briefing_prompt_and_persists_snapshots_r4_r5_r11() {
    let tmp = TempDir::new().unwrap();
    let store = SqliteStore::open_in_memory().unwrap();
    seed_volume(&store, "v-main", tmp.path(), None);
    store.add_root(tmp.path()).unwrap();

    let prj_root = tmp.path().join("Projects\\omega");
    seed_project_collection(&store, "col-omega", "omega", "v-main", &prj_root);
    let now = aegis_core::now_ms();
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\omega\\a.rs", "fn a() {}", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\omega\\b.rs", "fn b() {}", now);
    seed_file_with_mtime(&store, "v-main", tmp.path(), "Projects\\omega\\c.rs", "fn c() {}", now);

    let prep = prepare_cdo_briefing_prompt(
        store.conn(),
        30,
        false,
        &VaultPostureContext::default(),
        false,
        false,
    )
    .unwrap();

    assert!(prep.citations.len() >= 6);
    assert!(prep.citations.iter().any(|c| c.source_kind == "pillar_score"));
    assert!(prep.citations.iter().any(|c| c.source_kind == "bottleneck"));
    assert!(prep.citations.iter().any(|c| c.source_kind == "project_pulse"));
    assert!(prep.system_prompt.contains("K1"));
    assert!(prep.user_prompt.contains("R5/R11"));

    let snap = CdoExecutiveBriefing {
        id: aegis_core::new_id(),
        horizon_days: prep.horizon_days,
        executive_score: prep.executive_score,
        headline: prep.headline,
        summary_markdown: "Executive memo grounded in [1] and [6].".into(),
        citations: prep.citations,
        model_id: "extractive".into(),
        model_name: "Built-in Extractive".into(),
        backend: "extractive".into(),
        created_at: now,
    };
    save_cdo_briefing_snapshot(store.conn(), &snap).unwrap();

    let listed = list_cdo_briefings(store.conn(), 5).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, snap.id);

    let dash = build_cdo_dashboard_with_drives(
        store.conn(),
        &CdoScanOptions::default(),
        &VaultPostureContext::default(),
        false,
        vec![],
    )
    .unwrap();
    assert_eq!(dash.latest_briefing.as_ref().unwrap().id, snap.id);
}
