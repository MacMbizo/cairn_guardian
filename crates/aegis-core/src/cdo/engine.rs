//! Executive CDO synthesis, bottleneck diagnosis, reversible `K1` ratification,
//! and grounded executive briefing assembly (`5.2`, `K1`–`K5`, `R4`–`R9`, `R11`, `D1`).

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rusqlite::{Connection, OptionalExtension, params};

use crate::cross_project::{CrossProjectOptions, scan_cross_project_intelligence};
use crate::db::SqliteStore;
use crate::db::actions::health_breakdown;
use crate::db::audit::append_audit_in_tx;
use crate::engine::Screen;
use crate::error::{AegisError, Result};
use crate::folderless::{
    compile_intent_deterministic, delete_folderless_rule, save_folderless_rule,
};
use crate::governance::posture::{
    VaultPostureContext, build_privacy_posture, build_privacy_posture_with_drives,
};
use crate::governance::{
    DriveEncryptionStatus, GovernancePrivacyRuleInput, GovernanceRuleAction, delete_privacy_rule,
    save_privacy_rule,
};
use crate::healing::{HealScanOptions, scan_workspace_healing};
use crate::{new_id, now_ms};

use super::patterns::{analyze_project_pulses, compute_velocity_snapshot};
use super::{
    CdoBottleneck, CdoBottleneckKind, CdoBriefingCitation, CdoDashboard, CdoExecutiveBriefing,
    CdoPillarId, CdoPillarScore, CdoProjectPulse, CdoProposalAction, CdoRatificationRecord,
    CdoRecommendation, CdoRecommendationStatus, CdoScanOptions, CdoSeverity, PreparedCdoBriefing,
};

/// Origin and audit actor for Capability #14 AI Chief Digital Officer recommendations.
pub const CDO_AGENT: &str = "cdo_agent";

fn pillar_status(score: f64) -> String {
    if score >= 85.0 {
        "healthy".into()
    } else if score >= 65.0 {
        "attention".into()
    } else {
        "critical".into()
    }
}

fn executive_grade(score: f64) -> String {
    if score >= 92.0 {
        "A".into()
    } else if score >= 85.0 {
        "A-".into()
    } else if score >= 78.0 {
        "B+".into()
    } else if score >= 70.0 {
        "B".into()
    } else if score >= 60.0 {
        "C".into()
    } else {
        "D".into()
    }
}

/// Computes the live 5-pillar CDO dashboard using live BitLocker telemetry when requested.
pub fn build_cdo_dashboard(
    conn: &Connection,
    opts: &CdoScanOptions,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    live_probe_bitlocker: bool,
) -> Result<CdoDashboard> {
    build_cdo_dashboard_inner(conn, opts, vault_ctx, reveal_sensitive, live_probe_bitlocker, None)
}

/// Computes the 5-pillar CDO dashboard with injectable drive encryption states (fast deterministic tests).
pub fn build_cdo_dashboard_with_drives(
    conn: &Connection,
    opts: &CdoScanOptions,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    drives_override: Vec<DriveEncryptionStatus>,
) -> Result<CdoDashboard> {
    build_cdo_dashboard_inner(
        conn,
        opts,
        vault_ctx,
        reveal_sensitive,
        false,
        Some(drives_override),
    )
}

fn load_recommendation_states(
    conn: &Connection,
) -> Result<HashMap<String, (CdoRecommendationStatus, String)>> {
    let mut map = HashMap::new();
    let mut st = conn.prepare("SELECT rec_key, status, note FROM cdo_recommendation_state")?;
    for r in st.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (key, status, note) = r?;
        map.insert(key, (CdoRecommendationStatus::from_db(&status), note));
    }
    Ok(map)
}

fn first_exposed_inbox_parent(conn: &Connection) -> Result<Option<String>> {
    let mut st = conn.prepare(
        "SELECT f.path
         FROM security_findings sf
         JOIN security_finding_locations sfl ON sfl.finding_id = sf.id
         JOIN files f ON f.id = sfl.file_id
         JOIN volumes v ON v.id = f.volume_id
         WHERE sf.status = 'open'
           AND f.deleted_at IS NULL
           AND f.state NOT IN ('quarantine', 'missing')
           AND COALESCE(v.role, 'unassigned') != 'backup'
         ORDER BY sf.context_risk_score DESC, sf.created_at DESC
         LIMIT 16",
    )?;
    for row in st.query_map([], |r| r.get::<_, String>(0))? {
        let p = row?;
        let low = p.to_ascii_lowercase().replace('\\', "/");
        if (low.contains("/downloads/") || low.contains("/desktop/") || low.contains("/temp/"))
            && let Some(parent) = Path::new(&p).parent()
        {
            return Ok(Some(parent.to_string_lossy().to_string()));
        }
    }
    Ok(None)
}

fn rule_covers_prefix(conn: &Connection, prefix: &str) -> Result<bool> {
    let norm_pref = prefix.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
    let mut st = conn.prepare(
        "SELECT path_prefix FROM governance_privacy_rules WHERE enabled = 1 AND path_prefix IS NOT NULL",
    )?;
    for r in st.query_map([], |row| row.get::<_, String>(0))? {
        let existing = r?.trim().trim_end_matches(['\\', '/']).to_ascii_lowercase();
        if !existing.is_empty() && (norm_pref == existing || norm_pref.starts_with(&existing)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn build_cdo_dashboard_inner(
    conn: &Connection,
    opts: &CdoScanOptions,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    live_probe_bitlocker: bool,
    drives_override: Option<Vec<DriveEncryptionStatus>>,
) -> Result<CdoDashboard> {
    if opts.include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before viewing sensitive CDO evidence (R7)."
                .into(),
        ));
    }
    let started = Instant::now();
    let now = now_ms();
    let horizon_days = opts.horizon_days.unwrap_or(30).clamp(7, 365);
    let allow_sensitive = opts.include_sensitive && reveal_sensitive;

    let health = health_breakdown(conn)?;
    let posture = match drives_override {
        Some(drives) => build_privacy_posture_with_drives(conn, vault_ctx, drives)?,
        None => build_privacy_posture(conn, vault_ctx, live_probe_bitlocker)?,
    };
    let heal = scan_workspace_healing(
        conn,
        &HealScanOptions {
            scope_root: None,
            include_sensitive: allow_sensitive,
            include_dismissed: false,
            limit_notes: Some(150),
        },
        reveal_sensitive,
    )?;
    let cross = scan_cross_project_intelligence(
        conn,
        &CrossProjectOptions {
            project_id: None,
            kind: None,
            min_similarity: None,
            include_design_sets: Some(false),
            include_dismissed: Some(false),
            include_sensitive: Some(allow_sensitive),
            limit_findings: Some(25),
            limit_edges: Some(25),
        },
        reveal_sensitive,
    )?;

    let (project_pulses, excl_proj) =
        analyze_project_pulses(conn, now, horizon_days, allow_sensitive)?;
    let (velocity, loose_invoice_paths, excl_vel) =
        compute_velocity_snapshot(conn, now, horizon_days, allow_sensitive)?;

    let storage_backup_score = health
        .dimensions
        .iter()
        .take(2)
        .map(|d| d.score)
        .sum::<f64>()
        / health.dimensions.len().clamp(1, 2) as f64;
    let security_privacy_score = posture.privacy_score;
    let workspace_hygiene_score = ((heal.relational_integrity_score
        + heal.taxonomic_hygiene_score
        + health
            .dimensions
            .get(2)
            .map_or(health.overall_score, |d| d.score)
        + health
            .dimensions
            .get(3)
            .map_or(health.overall_score, |d| d.score))
        / 4.0)
        .clamp(10.0, 100.0);

    let lagging_projects: Vec<&CdoProjectPulse> = project_pulses
        .iter()
        .filter(|p| p.health_flag == "doc_lagging")
        .collect();
    let knowledge_alignment_score = (100.0
        - (lagging_projects.len() as f64) * 14.0
        - (cross.conflict_count as f64) * 12.0
        - (cross.duplicate_effort_count as f64) * 6.0
        - ((velocity.pending_knowledge_proposals as f64) * 2.0).min(16.0))
    .clamp(15.0, 100.0);

    let automation_velocity_score = (60.0
        + velocity.filing_automation_ratio * 35.0
        + if velocity.active_folderless_rules > 0 { 5.0 } else { 0.0 }
        - ((velocity.loose_invoice_docs as f64) * 6.0).min(30.0)
        - ((velocity.loose_inbox_files as f64 - velocity.loose_invoice_docs as f64).max(0.0) * 1.5)
            .min(15.0)
        - ((velocity.pending_action_batches as f64) * 4.0).min(12.0))
    .clamp(15.0, 100.0);

    let round1 = |v: f64| (v * 10.0).round() / 10.0;
    let pillars = vec![
        CdoPillarScore {
            id: CdoPillarId::StorageAndBackup,
            label: "Storage & Backup".into(),
            score: round1(storage_backup_score),
            weight: 0.25,
            status: pillar_status(storage_backup_score),
            detail: health
                .dimensions
                .get(1)
                .map_or_else(|| "Drive & backup readiness".into(), |d| {
                    format!("{} ({} issues)", d.label, d.offenders_count)
                }),
            target_screen: Screen::Drives,
        },
        CdoPillarScore {
            id: CdoPillarId::SecurityAndPrivacy,
            label: "Security & Privacy".into(),
            score: round1(security_privacy_score),
            weight: 0.25,
            status: pillar_status(security_privacy_score),
            detail: format!(
                "{} open secrets/PII, {} active rules",
                posture.open_findings_count, posture.active_rules_count
            ),
            target_screen: Screen::Governance,
        },
        CdoPillarScore {
            id: CdoPillarId::WorkspaceHygiene,
            label: "Workspace Hygiene".into(),
            score: round1(workspace_hygiene_score),
            weight: 0.20,
            status: pillar_status(workspace_hygiene_score),
            detail: format!(
                "{} broken links, {} tag weeds",
                heal.broken_links_count, heal.tag_weeds_count
            ),
            target_screen: Screen::Healing,
        },
        CdoPillarScore {
            id: CdoPillarId::KnowledgeAndAlignment,
            label: "Knowledge & Alignment".into(),
            score: round1(knowledge_alignment_score),
            weight: 0.15,
            status: pillar_status(knowledge_alignment_score),
            detail: format!(
                "{} doc-lagging projects, {} cross-project conflicts",
                lagging_projects.len(),
                cross.conflict_count
            ),
            target_screen: Screen::CrossProject,
        },
        CdoPillarScore {
            id: CdoPillarId::AutomationVelocity,
            label: "Automation Velocity".into(),
            score: round1(automation_velocity_score),
            weight: 0.15,
            status: pillar_status(automation_velocity_score),
            detail: format!(
                "{} loose invoice/tax docs, {} active Folderless rules",
                velocity.loose_invoice_docs, velocity.active_folderless_rules
            ),
            target_screen: Screen::Folderless,
        },
    ];

    let executive_score = round1(pillars.iter().map(|p| p.score * p.weight).sum::<f64>());
    let executive_grade = executive_grade(executive_score);

    let mut bottlenecks: Vec<CdoBottleneck> = Vec::new();
    let mut recommendations: Vec<CdoRecommendation> = Vec::new();
    let triage_map = load_recommendation_states(conn)?;

    // 1. Documentation lag behind code commits (`5.2` example #1).
    if !lagging_projects.is_empty() {
        let names: Vec<String> = lagging_projects.iter().map(|p| p.name.clone()).collect();
        let max_lag = lagging_projects
            .iter()
            .filter_map(|p| p.doc_lag_days)
            .max()
            .unwrap_or(0);
        let sev = if lagging_projects.len() >= 2 || max_lag >= 21 {
            CdoSeverity::High
        } else {
            CdoSeverity::Medium
        };
        bottlenecks.push(CdoBottleneck {
            id: "bn-doc-lag".into(),
            kind: CdoBottleneckKind::DocLagBehindCode,
            severity: sev,
            title: format!(
                "Documentation lags behind active code across {} project(s)",
                lagging_projects.len()
            ),
            diagnosis: format!(
                "Project files in {} show active source edits while README/spec docs are older or absent (up to {}d lag).",
                names.join(", "),
                max_lag
            ),
            impact_metric: format!("{} project(s) with doc lag", lagging_projects.len()),
            affected_projects: names.clone(),
            evidence_paths: Vec::new(),
            recommended_screen: Screen::CrossProject,
        });
        recommendations.push(CdoRecommendation {
            rec_key: "cdo:doc-lag:review".into(),
            title: "Review documentation lag in Cross-Project & Knowledge Base".into(),
            rationale: format!(
                "Refresh README/design notes and extract recent decisions across {}.",
                names.join(", ")
            ),
            bottleneck_kind: CdoBottleneckKind::DocLagBehindCode,
            severity: sev,
            status: CdoRecommendationStatus::Open,
            owner_note: String::new(),
            can_ratify_directly: false,
            proposal: CdoProposalAction::NavigateToScreen {
                screen: Screen::CrossProject,
                label: "Open Cross-Project Intelligence".into(),
            },
        });
    }

    // 2. Manual invoice / receipt inbox filing friction (`5.2` example #2 + `R6`).
    if velocity.loose_invoice_docs >= 3
        || (velocity.loose_inbox_files >= 8 && velocity.active_folderless_rules == 0)
    {
        let roots: Vec<String> = conn
            .query_row(
                "SELECT value_json FROM settings WHERE key = 'roots' AND scope = 'global'",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .and_then(|json| {
                serde_json::from_str::<Vec<crate::contracts::RootConfig>>(&json).ok()
            })
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.enabled)
            .map(|r| r.path)
            .collect();
        let intent = if velocity.loose_invoice_docs >= 3 {
            "Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name"
        } else {
            "Route loose documents and receipts in Downloads and Desktop to Filing Cabinet by year and document type"
        };
        let mut draft = compile_intent_deterministic(intent, &roots)?.rule;
        draft.name = if velocity.loose_invoice_docs >= 3 {
            "CDO Proposal: Auto-file Loose Invoices".into()
        } else {
            "CDO Proposal: Tame Inbox Clutter".into()
        };
        let sev = if velocity.loose_invoice_docs >= 5 {
            CdoSeverity::High
        } else {
            CdoSeverity::Medium
        };
        bottlenecks.push(CdoBottleneck {
            id: "bn-manual-filing".into(),
            kind: CdoBottleneckKind::ManualFilingFriction,
            severity: sev,
            title: format!(
                "{} unfiled invoice/financial document(s) sitting in inbox folders",
                velocity.loose_invoice_docs
            ),
            diagnosis: format!(
                "{} loose files remain in Downloads/Desktop ({} invoice/receipt/statement items).",
                velocity.loose_inbox_files, velocity.loose_invoice_docs
            ),
            impact_metric: format!("{} inbox invoices", velocity.loose_invoice_docs),
            affected_projects: Vec::new(),
            evidence_paths: loose_invoice_paths,
            recommended_screen: Screen::Folderless,
        });
        recommendations.push(CdoRecommendation {
            rec_key: "cdo:folderless:invoice-automation".into(),
            title: format!("Activate Folderless Rule: {}", draft.name),
            rationale: format!(
                "Compiles a visible, reversible rule (`R6`) targeting `{}` without moving anything until you review batches in Simulator (`K1`).",
                draft.dest_root
            ),
            bottleneck_kind: CdoBottleneckKind::ManualFilingFriction,
            severity: sev,
            status: CdoRecommendationStatus::Open,
            owner_note: String::new(),
            can_ratify_directly: true,
            proposal: CdoProposalAction::CreateFolderlessRule {
                draft: Box::new(draft),
            },
        });
    }

    // 3. Exposed secrets / PII without inbox privacy rule (`R7`).
    if posture.open_findings_count > 0 {
        let sev = if posture.open_findings_count >= 3 {
            CdoSeverity::Critical
        } else {
            CdoSeverity::High
        };
        bottlenecks.push(CdoBottleneck {
            id: "bn-security-debt".into(),
            kind: CdoBottleneckKind::SecurityExposureDebt,
            severity: sev,
            title: format!(
                "{} open PII/secret exposure(s) require R7 vaulting or quarantine",
                posture.open_findings_count
            ),
            diagnosis: format!(
                "Personal Data Governance detected {} untriaged credential or PII finding(s) across indexed files.",
                posture.open_findings_count
            ),
            impact_metric: format!("{} open findings", posture.open_findings_count),
            affected_projects: Vec::new(),
            evidence_paths: Vec::new(),
            recommended_screen: Screen::Governance,
        });

        let exposed_dir = first_exposed_inbox_parent(conn)?;
        let uncovered_dir = match exposed_dir {
            Some(dir) if !rule_covers_prefix(conn, &dir)? => Some(dir),
            _ => None,
        };
        if let Some(dir) = uncovered_dir {
            recommendations.push(CdoRecommendation {
                rec_key: format!("cdo:governance:protect-inbox:{}", dir.to_ascii_lowercase()),
                title: "Protect exposed inbox credential files with automatic `secret` privacy rule"
                    .into(),
                rationale: format!(
                    "Creates a visible Governance privacy rule covering `{dir}` (`*.env*`) so credential stores are masked and withheld from AI prompts (`R7`)."
                ),
                bottleneck_kind: CdoBottleneckKind::SecurityExposureDebt,
                severity: sev,
                status: CdoRecommendationStatus::Open,
                owner_note: String::new(),
                can_ratify_directly: true,
                proposal: CdoProposalAction::CreatePrivacyRule {
                    draft: GovernancePrivacyRuleInput {
                        id: None,
                        name: "CDO Proposal: Guard Inbox Secrets".into(),
                        path_prefix: Some(dir),
                        filename_pattern: Some("*.env*".into()),
                        rule_action: GovernanceRuleAction::AutoSecret,
                        enabled: Some(true),
                    },
                },
            });
        } else {
            recommendations.push(CdoRecommendation {
                rec_key: "cdo:governance:triage-findings".into(),
                title: "Triage or Vault open secret findings in Data Governance".into(),
                rationale:
                    "Review masked exposures and stage exposed copies for 30-day Quarantine (`K1`/`K2`)."
                        .into(),
                bottleneck_kind: CdoBottleneckKind::SecurityExposureDebt,
                severity: sev,
                status: CdoRecommendationStatus::Open,
                owner_note: String::new(),
                can_ratify_directly: false,
                proposal: CdoProposalAction::NavigateToScreen {
                    screen: Screen::Governance,
                    label: "Open Data Governance".into(),
                },
            });
        }
    }

    // 4. Self-healing broken link / tag taxonomy drift (`4.1`).
    if heal.broken_links_count > 0 || heal.tag_weeds_count > 0 || heal.rule_path_issues_count > 0 {
        let total_heal =
            heal.broken_links_count + heal.tag_weeds_count + heal.rule_path_issues_count;
        let sev = if heal.broken_links_count >= 5 {
            CdoSeverity::High
        } else {
            CdoSeverity::Medium
        };
        bottlenecks.push(CdoBottleneck {
            id: "bn-healing-drift".into(),
            kind: CdoBottleneckKind::LinkAndTaxonomyDrift,
            severity: sev,
            title: format!(
                "{total_heal} broken link(s), tag variants, or stale rule paths"
            ),
            diagnosis: format!(
                "Self-Healing found {} broken Markdown/wikilinks, {} tag weed clusters, and {} stale Folderless rule path(s).",
                heal.broken_links_count, heal.tag_weeds_count, heal.rule_path_issues_count
            ),
            impact_metric: format!("{total_heal} workspace repairs pending"),
            affected_projects: Vec::new(),
            evidence_paths: Vec::new(),
            recommended_screen: Screen::Healing,
        });
        recommendations.push(CdoRecommendation {
            rec_key: "cdo:healing:review-patches".into(),
            title: "Preview atomic Self-Healing repairs in Simulator (`K1`)".into(),
            rationale:
                "Stage verified TOCTOU-guarded `PatchText` repairs so links stay intact after moves."
                    .into(),
            bottleneck_kind: CdoBottleneckKind::LinkAndTaxonomyDrift,
            severity: sev,
            status: CdoRecommendationStatus::Open,
            owner_note: String::new(),
            can_ratify_directly: false,
            proposal: CdoProposalAction::NavigateToScreen {
                screen: Screen::Healing,
                label: "Open Self-Healing Workspace".into(),
            },
        });
    }

    // 5. Cross-project conflicts / duplicate research (`3.3`).
    if cross.conflict_count > 0 || cross.duplicate_effort_count > 0 {
        let sev = if cross.conflict_count > 0 {
            CdoSeverity::High
        } else {
            CdoSeverity::Medium
        };
        bottlenecks.push(CdoBottleneck {
            id: "bn-cross-project".into(),
            kind: CdoBottleneckKind::CrossProjectFriction,
            severity: sev,
            title: format!(
                "{} decision/stack conflict(s) and {} duplicate effort overlap(s)",
                cross.conflict_count, cross.duplicate_effort_count
            ),
            diagnosis:
                "Multiple project folders show overlapping research documents or conflicting architectural decisions."
                    .into(),
            impact_metric: format!(
                "{} cross-project insights",
                cross.conflict_count + cross.duplicate_effort_count
            ),
            affected_projects: Vec::new(),
            evidence_paths: Vec::new(),
            recommended_screen: Screen::CrossProject,
        });
    }

    // 6. Pending K1 queues / Knowledge backlog.
    if velocity.pending_action_batches > 0 || velocity.pending_knowledge_proposals >= 5 {
        bottlenecks.push(CdoBottleneck {
            id: "bn-pending-queues".into(),
            kind: CdoBottleneckKind::PendingReviewQueue,
            severity: CdoSeverity::Low,
            title: format!(
                "{} pending Simulator batch(es) and {} Knowledge proposal(s) awaiting decision",
                velocity.pending_action_batches, velocity.pending_knowledge_proposals
            ),
            diagnosis:
                "Clearing draft review queues unlocks cleaner automation metrics and up-to-date project knowledge."
                    .into(),
            impact_metric: format!(
                "{} awaiting review",
                velocity.pending_action_batches + velocity.pending_knowledge_proposals
            ),
            affected_projects: Vec::new(),
            evidence_paths: Vec::new(),
            recommended_screen: Screen::Simulator,
        });
    }

    // 7. Weekly Note schedule optimization when unconfigured and bottlenecks exist (`5.2` strategic schedule adjustment).
    let note_day_set: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM settings WHERE key = 'engine.note_day' AND scope = 'global')",
            [],
            |r| r.get(0),
        )?;
    if !note_day_set && !bottlenecks.is_empty() {
        recommendations.push(CdoRecommendation {
            rec_key: "cdo:schedule:weekly-digest-friday".into(),
            title: "Align Weekly Executive Digest to Friday afternoon (16:00)".into(),
            rationale:
                "Sets `engine.note_day = 4` (Friday) so operational digests arrive before weekend shutdown; completely reversible via Undo (`K1`)."
                    .into(),
            bottleneck_kind: CdoBottleneckKind::PendingReviewQueue,
            severity: CdoSeverity::Low,
            status: CdoRecommendationStatus::Open,
            owner_note: String::new(),
            can_ratify_directly: true,
            proposal: CdoProposalAction::AdjustSetting {
                key: "engine.note_day".into(),
                scope: "global".into(),
                value_json: "4".into(),
                display_label: "Set Weekly Digest day to Friday (4)".into(),
            },
        });
    }

    // Apply persisted triage overlay & optional dismissal filter.
    let mut final_recs = Vec::new();
    for mut rec in recommendations {
        if let Some((st, note)) = triage_map.get(&rec.rec_key) {
            rec.status = *st;
            rec.owner_note = note.clone();
        }
        if rec.status == CdoRecommendationStatus::Dismissed && !opts.include_dismissed {
            continue;
        }
        final_recs.push(rec);
    }

    bottlenecks.sort_by_key(|a| std::cmp::Reverse(a.severity.rank()));
    final_recs.sort_by(|a, b| {
        let rank_status = |s: CdoRecommendationStatus| match s {
            CdoRecommendationStatus::Open => 0,
            CdoRecommendationStatus::Snoozed => 1,
            CdoRecommendationStatus::Ratified => 2,
            CdoRecommendationStatus::Dismissed => 3,
        };
        rank_status(a.status)
            .cmp(&rank_status(b.status))
            .then_with(|| b.severity.rank().cmp(&a.severity.rank()))
    });

    let recent_ratifications = list_cdo_ratifications(conn, 15)?;
    let latest_briefing = list_cdo_briefings(conn, 1)?.into_iter().next();

    let posture_summary = format!(
        "Executive score {executive_score}/100 ({executive_grade}) across {} project(s): {} bottleneck(s) and {} strategic recommendation(s) ready for owner review.",
        project_pulses.len(),
        bottlenecks.len(),
        final_recs
            .iter()
            .filter(|r| r.status == CdoRecommendationStatus::Open)
            .count()
    );

    Ok(CdoDashboard {
        executive_score,
        executive_grade,
        posture_summary,
        pillars,
        velocity,
        project_pulses,
        bottlenecks,
        recommendations: final_recs,
        recent_ratifications,
        latest_briefing,
        excluded_sensitive_count: excl_proj + excl_vel + heal.excluded_sensitive_count,
        generated_at: now,
        took_ms: started.elapsed().as_millis() as i64,
    })
}

/// Persists owner triage decision (`open`, `ratified`, `snoozed`, `dismissed`) across rescans (`K1`).
pub fn set_cdo_recommendation_status(
    store: &mut SqliteStore,
    rec_key: &str,
    status: CdoRecommendationStatus,
    note: Option<&str>,
) -> Result<()> {
    let key = rec_key.trim();
    if key.is_empty() {
        return Err(AegisError::InvalidInput("rec_key must not be empty.".into()));
    }
    let now = now_ms();
    let note_str = note.unwrap_or("").trim().to_string();
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "INSERT INTO cdo_recommendation_state (rec_key, status, note, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(rec_key) DO UPDATE SET status = excluded.status, note = excluded.note, updated_at = excluded.updated_at",
            params![key, status.as_str(), note_str, now],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "cdo.triage_recommendation",
            key,
            &serde_json::json!({ "status": status.as_str(), "note": note_str }).to_string(),
        )?;
        Ok(())
    })
}

/// Ratifies and executes one structural CDO proposal with full `K1` undo snapshot.
pub fn ratify_cdo_recommendation(
    store: &mut SqliteStore,
    rec: &CdoRecommendation,
) -> Result<CdoRatificationRecord> {
    let id = new_id();
    let now = now_ms();

    let (action_kind, target_ref, _previous_json, applied_summary) =
        store.with_full_sync_tx(|tx| {
            let result = match &rec.proposal {
                CdoProposalAction::CreateFolderlessRule { draft } => {
                    let saved = save_folderless_rule(tx, draft)?;
                    (
                        "create_folderless_rule".to_string(),
                        saved.id.clone(),
                        None,
                        format!("Created visible Folderless rule `{}` -> `{}`", saved.name, saved.dest_root),
                    )
                }
                CdoProposalAction::CreatePrivacyRule { draft } => {
                    let saved = save_privacy_rule(tx, draft)?;
                    (
                        "create_privacy_rule".to_string(),
                        saved.id.clone(),
                        None,
                        format!("Created Privacy Governance rule `{}`", saved.name),
                    )
                }
                CdoProposalAction::AdjustSetting {
                    key,
                    scope,
                    value_json,
                    display_label,
                } => {
                    let prev: Option<String> = tx
                        .query_row(
                            "SELECT value_json FROM settings WHERE key = ?1 AND scope = ?2",
                            params![key, scope],
                            |r| r.get(0),
                        )
                        .optional()?;
                    tx.execute(
                        "INSERT INTO settings (key, scope, value_json, updated_at)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT(key, scope) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                        params![key, scope, value_json, now],
                    )?;
                    let snap = serde_json::json!({
                        "key": key,
                        "scope": scope,
                        "previous_value_json": prev,
                    })
                    .to_string();
                    (
                        "adjust_setting".to_string(),
                        format!("{scope}::{key}"),
                        Some(snap),
                        display_label.clone(),
                    )
                }
                CdoProposalAction::NavigateToScreen { .. } => {
                    return Err(AegisError::InvalidInput(
                        "Navigation recommendations open the target screen directly instead of ratifying.".into(),
                    ));
                }
            };

            tx.execute(
                "INSERT INTO cdo_ratifications
                 (id, rec_key, rec_title, action_kind, target_ref, previous_json, applied_summary, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'applied', ?8)",
                params![
                    id,
                    rec.rec_key,
                    rec.title,
                    result.0,
                    result.1,
                    result.2,
                    result.3,
                    now
                ],
            )?;
            tx.execute(
                "INSERT INTO cdo_recommendation_state (rec_key, status, note, updated_at)
                 VALUES (?1, 'ratified', '', ?2)
                 ON CONFLICT(rec_key) DO UPDATE SET status = 'ratified', updated_at = excluded.updated_at",
                params![rec.rec_key, now],
            )?;
            append_audit_in_tx(
                tx,
                "user",
                "cdo.ratify_recommendation",
                &id,
                &serde_json::json!({
                    "rec_key": rec.rec_key,
                    "action_kind": result.0,
                    "target_ref": result.1,
                })
                .to_string(),
            )?;
            Ok(result)
        })?;

    Ok(CdoRatificationRecord {
        id,
        rec_key: rec.rec_key.clone(),
        rec_title: rec.title.clone(),
        action_kind,
        target_ref,
        applied_summary,
        status: "applied".into(),
        created_at: now,
        undone_at: None,
        can_undo: true,
    })
}

/// Reverses one owner-ratified CDO rule or setting change (`K1` single-click undo).
pub fn undo_cdo_ratification(
    store: &mut SqliteStore,
    ratification_id: &str,
) -> Result<CdoRatificationRecord> {
    let now = now_ms();
    store.with_full_sync_tx(|tx| {
        struct RatificationRow {
            rec_key: String,
            rec_title: String,
            action_kind: String,
            target_ref: String,
            previous_json: Option<String>,
            applied_summary: String,
            status: String,
            created_at: i64,
        }
        let row: Option<RatificationRow> = tx
            .query_row(
                "SELECT rec_key, rec_title, action_kind, target_ref, previous_json, applied_summary, status, created_at
                 FROM cdo_ratifications WHERE id = ?1",
                params![ratification_id],
                |r| {
                    Ok(RatificationRow {
                        rec_key: r.get(0)?,
                        rec_title: r.get(1)?,
                        action_kind: r.get(2)?,
                        target_ref: r.get(3)?,
                        previous_json: r.get(4)?,
                        applied_summary: r.get(5)?,
                        status: r.get(6)?,
                        created_at: r.get(7)?,
                    })
                },
            )
            .optional()?;
        let Some(row) = row else {
            return Err(AegisError::NotFound(format!("CDO ratification {ratification_id}")));
        };
        if row.status != "applied" {
            return Err(AegisError::InvalidInput(format!(
                "Ratification {ratification_id} is already {}.",
                row.status
            )));
        }

        match row.action_kind.as_str() {
            "create_folderless_rule" => {
                let _ = delete_folderless_rule(tx, &row.target_ref)?;
            }
            "create_privacy_rule" => {
                let _ = delete_privacy_rule(tx, &row.target_ref)?;
            }
            "adjust_setting" => {
                let snap_str = row.previous_json.as_deref().ok_or_else(|| {
                    AegisError::InvalidInput("Missing prior setting snapshot for undo.".into())
                })?;
                let snap: serde_json::Value = serde_json::from_str(snap_str)?;
                let key = snap["key"].as_str().unwrap_or_default();
                let scope = snap["scope"].as_str().unwrap_or("global");
                match snap.get("previous_value_json").and_then(|v| v.as_str()) {
                    Some(prev_val) => {
                        tx.execute(
                            "INSERT INTO settings (key, scope, value_json, updated_at)
                             VALUES (?1, ?2, ?3, ?4)
                             ON CONFLICT(key, scope) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at",
                            params![key, scope, prev_val, now],
                        )?;
                    }
                    None => {
                        tx.execute(
                            "DELETE FROM settings WHERE key = ?1 AND scope = ?2",
                            params![key, scope],
                        )?;
                    }
                }
            }
            other => {
                return Err(AegisError::InvalidInput(format!(
                    "Unsupported ratification kind: {other}"
                )));
            }
        }

        tx.execute(
            "UPDATE cdo_ratifications SET status = 'undone', undone_at = ?1 WHERE id = ?2",
            params![now, ratification_id],
        )?;
        tx.execute(
            "INSERT INTO cdo_recommendation_state (rec_key, status, note, updated_at)
             VALUES (?1, 'open', '', ?2)
             ON CONFLICT(rec_key) DO UPDATE SET status = 'open', updated_at = excluded.updated_at",
            params![row.rec_key, now],
        )?;
        append_audit_in_tx(
            tx,
            "user",
            "cdo.undo_ratification",
            ratification_id,
            &serde_json::json!({ "rec_key": row.rec_key, "action_kind": row.action_kind }).to_string(),
        )?;

        Ok(CdoRatificationRecord {
            id: ratification_id.to_string(),
            rec_key: row.rec_key,
            rec_title: row.rec_title,
            action_kind: row.action_kind,
            target_ref: row.target_ref,
            applied_summary: row.applied_summary,
            status: "undone".into(),
            created_at: row.created_at,
            undone_at: Some(now),
            can_undo: false,
        })
    })
}

pub fn list_cdo_ratifications(conn: &Connection, limit: usize) -> Result<Vec<CdoRatificationRecord>> {
    let mut st = conn.prepare(
        "SELECT id, rec_key, rec_title, action_kind, target_ref, applied_summary, status, created_at, undone_at
         FROM cdo_ratifications
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;
    let rows = st.query_map(params![limit.max(1) as i64], |r| {
        let status: String = r.get(6)?;
        Ok(CdoRatificationRecord {
            id: r.get(0)?,
            rec_key: r.get(1)?,
            rec_title: r.get(2)?,
            action_kind: r.get(3)?,
            target_ref: r.get(4)?,
            applied_summary: r.get(5)?,
            can_undo: status == "applied",
            status,
            created_at: r.get(7)?,
            undone_at: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

pub fn list_cdo_briefings(conn: &Connection, limit: usize) -> Result<Vec<CdoExecutiveBriefing>> {
    let mut st = conn.prepare(
        "SELECT id, horizon_days, executive_score, headline, summary_markdown, citations_json,
                model_id, model_name, backend, created_at
         FROM cdo_briefing_snapshots
         ORDER BY created_at DESC
         LIMIT ?1",
    )?;
    let rows = st.query_map(params![limit.max(1) as i64], |r| {
        let cit_json: String = r.get(5)?;
        let citations: Vec<CdoBriefingCitation> =
            serde_json::from_str(&cit_json).unwrap_or_default();
        Ok(CdoExecutiveBriefing {
            id: r.get(0)?,
            horizon_days: r.get(1)?,
            executive_score: r.get(2)?,
            headline: r.get(3)?,
            summary_markdown: r.get(4)?,
            citations,
            model_id: r.get(6)?,
            model_name: r.get(7)?,
            backend: r.get(8)?,
            created_at: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
}

/// Builds a cited prompt for off-lock `OnDemandLlm` synthesis strictly from disk & catalog data (`R4`, `R5`, `R7`, `R11`).
pub fn prepare_cdo_briefing_prompt(
    conn: &Connection,
    horizon_days: i64,
    include_sensitive: bool,
    vault_ctx: &VaultPostureContext,
    reveal_sensitive: bool,
    live_probe_bitlocker: bool,
) -> Result<PreparedCdoBriefing> {
    if include_sensitive && !reveal_sensitive {
        return Err(AegisError::PermissionDenied(
            "Windows Hello / Windows user verification is required before including sensitive files in executive briefings (R7)."
                .into(),
        ));
    }
    let dash = build_cdo_dashboard(
        conn,
        &CdoScanOptions {
            horizon_days: Some(horizon_days),
            include_sensitive,
            include_dismissed: false,
        },
        vault_ctx,
        reveal_sensitive,
        live_probe_bitlocker,
    )?;

    let mut citations = Vec::new();
    let mut facts = String::new();

    for (idx, p) in dash.pillars.iter().enumerate() {
        let ref_id = idx + 1;
        citations.push(CdoBriefingCitation {
            ref_id,
            title: format!("Pillar: {}", p.label),
            source_kind: "pillar_score".into(),
            detail: format!("{}/100 ({}) — {}", p.score, p.status, p.detail),
        });
        facts.push_str(&format!(
            "[{}] {} = {}/100 ({}) — {}\n",
            ref_id, p.label, p.score, p.status, p.detail
        ));
    }

    for bn in dash.bottlenecks.iter().take(6) {
        let ref_id = citations.len() + 1;
        citations.push(CdoBriefingCitation {
            ref_id,
            title: bn.title.clone(),
            source_kind: "bottleneck".into(),
            detail: format!("{} ({})", bn.diagnosis, bn.impact_metric),
        });
        facts.push_str(&format!(
            "[{}] Bottleneck ({:?}): {} — {}\n",
            ref_id, bn.severity, bn.title, bn.diagnosis
        ));
    }

    for pulse in dash.project_pulses.iter().take(6) {
        let ref_id = citations.len() + 1;
        let lag_txt = pulse
            .doc_lag_days
            .map_or_else(|| "n/a".into(), |d| format!("{d}d lag"));
        let detail = format!(
            "code={}, docs={}, 30d_active={}, lag={}, flag={}",
            pulse.code_files, pulse.doc_files, pulse.recent_30d_files, lag_txt, pulse.health_flag
        );
        citations.push(CdoBriefingCitation {
            ref_id,
            title: format!("Project: {}", pulse.name),
            source_kind: "project_pulse".into(),
            detail: detail.clone(),
        });
        facts.push_str(&format!("[{}] Project `{}`: {}\n", ref_id, pulse.name, detail));
    }

    let headline = if let Some(top) = dash.bottlenecks.first() {
        format!(
            "Executive Score {}/100 ({}) — Top priority: {}",
            dash.executive_score, dash.executive_grade, top.title
        )
    } else {
        format!(
            "Executive Score {}/100 ({}) — Workspace operations aligned",
            dash.executive_score, dash.executive_grade
        )
    };

    let system_prompt = "You are Aegis Guardian's local Chief Digital Officer (CDO) Agent. Synthesize a crisp, practical executive memo grounded strictly in the numbered workspace facts provided. Cite evidence with [1], [2], etc. Never claim screen monitoring or autonomous file changes outside K1 owner ratification.".to_string();
    let user_prompt = format!(
        "Workspace Executive Horizon: {} days\nOverall Executive Score: {}/100 ({})\nObservation Policy: {}\n\nGrounded Evidence:\n{}\nWrite: (1) Executive Summary, (2) Key Bottlenecks, (3) Recommended Next Actions for the Owner.",
        dash.velocity.horizon_days,
        dash.executive_score,
        dash.executive_grade,
        dash.velocity.observation_mode,
        facts
    );

    Ok(PreparedCdoBriefing {
        horizon_days: dash.velocity.horizon_days,
        executive_score: dash.executive_score,
        headline,
        system_prompt,
        user_prompt,
        citations,
    })
}

pub fn save_cdo_briefing_snapshot(
    conn: &Connection,
    briefing: &CdoExecutiveBriefing,
) -> Result<()> {
    let cit_json = serde_json::to_string(&briefing.citations)?;
    conn.execute(
        "INSERT INTO cdo_briefing_snapshots
         (id, horizon_days, executive_score, headline, summary_markdown, citations_json,
          model_id, model_name, backend, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            briefing.id,
            briefing.horizon_days,
            briefing.executive_score,
            briefing.headline,
            briefing.summary_markdown,
            cit_json,
            briefing.model_id,
            briefing.model_name,
            briefing.backend,
            briefing.created_at
        ],
    )?;
    // Retain latest 10 executive briefing snapshots.
    conn.execute(
        "DELETE FROM cdo_briefing_snapshots
         WHERE id NOT IN (
             SELECT id FROM cdo_briefing_snapshots ORDER BY created_at DESC LIMIT 10
         )",
        [],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteStore;

    #[test]
    fn empty_store_builds_clean_5_pillar_dashboard() {
        let store = SqliteStore::open_in_memory().unwrap();
        let dash = build_cdo_dashboard_with_drives(
            store.conn(),
            &CdoScanOptions::default(),
            &VaultPostureContext::default(),
            false,
            vec![],
        )
        .unwrap();
        assert_eq!(dash.pillars.len(), 5);
        assert!(dash.executive_score >= 0.0 && dash.executive_score <= 100.0);
        assert!(dash.velocity.observation_mode.contains("R5/R11"));
    }
}
