//! Typed IPC contracts aligned 1:1 with `C1_IPC_Action_Contracts.md` (v1.2).

use crate::error::IpcError;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TargetKind {
    File,
    Dir,
    AtomicUnit,
}

impl TargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::AtomicUnit => "atomic_unit",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "dir" => Self::Dir,
            "atomic_unit" => Self::AtomicUnit,
            _ => Self::File,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ActionType {
    Move,
    Copy,
    Rename,
    Quarantine,
    Purge,
    Archive,
    Restore,
}

impl ActionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Copy => "copy",
            Self::Rename => "rename",
            Self::Quarantine => "quarantine",
            Self::Purge => "purge",
            Self::Archive => "archive",
            Self::Restore => "restore",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "move" => Some(Self::Move),
            "copy" => Some(Self::Copy),
            "rename" => Some(Self::Rename),
            "quarantine" => Some(Self::Quarantine),
            "purge" => Some(Self::Purge),
            "archive" => Some(Self::Archive),
            "restore" => Some(Self::Restore),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ActionStatus {
    Proposed,
    Approved,
    Executed,
    Failed,
    Skipped,
    RolledBack,
    Rejected,
    Superseded,
    Expired,
    UnrecoverableMissing,
}

impl ActionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Approved => "approved",
            Self::Executed => "executed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::RolledBack => "rolled_back",
            Self::Rejected => "rejected",
            Self::Superseded => "superseded",
            Self::Expired => "expired",
            Self::UnrecoverableMissing => "unrecoverable_missing",
        }
    }

    pub fn from_db(s: &str) -> Self {
        match s {
            "approved" => Self::Approved,
            "executed" => Self::Executed,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            "rolled_back" => Self::RolledBack,
            "rejected" => Self::Rejected,
            "superseded" => Self::Superseded,
            "expired" => Self::Expired,
            "unrecoverable_missing" => Self::UnrecoverableMissing,
            _ => Self::Proposed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BatchStatus {
    Draft,
    Approved,
    Executing,
    Executed,
    Partial,
    RolledBack,
    Failed,
}

impl BatchStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::Executing => "executing",
            Self::Executed => "executed",
            Self::Partial => "partial",
            Self::RolledBack => "rolled_back",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RollbackConflictKind {
    Modified,
    MissingTarget,
    Collision,
}

impl RollbackConflictKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modified => "modified",
            Self::MissingTarget => "missing_target",
            Self::Collision => "collision",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "modified" => Some(Self::Modified),
            "missing_target" => Some(Self::MissingTarget),
            "collision" => Some(Self::Collision),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConflictResolutionChoice {
    RevertKeepingEdits,
    CopyBackKeepingBoth,
    RestoreAsRename,
    Skip,
    AbortBatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LifecycleState {
    Active,
    Warm,
    Cold,
    Archive,
}

impl LifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Warm => "warm",
            Self::Cold => "cold",
            Self::Archive => "archive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HardwareProfile {
    Eco,
    Standard,
    Pro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AutonomyDial {
    Manual,
    Suggest,
    AutoApproved,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ActionClass {
    Refile,
    Archive,
    Quarantine,
    Purge,
    SecurityRemediate,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RootConfig {
    pub id: String,
    pub path: String,
    pub enabled: bool,
    pub volume_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VolumeInfo {
    pub id: String,
    pub mount_point: String,
    pub fs_type: String,
    pub is_removable: bool,
    pub is_readonly: bool,
    pub has_stable_inode: bool,
    pub case_sensitive: bool,
    pub quarantine_dir: Option<String>,
    pub status: String,
    #[ts(type = "number")]
    pub last_seen: i64,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub free_bytes: u64,
    pub label: Option<String>,
    /// `unassigned` | `general` | `backup` (see [`DriveRole`]).
    pub role: String,
    pub passport_id: Option<String>,
    #[ts(type = "number | null")]
    pub last_catalog_complete_at: Option<i64>,
}

/// What a drive is for (Drives & Duplicates spec §7.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DriveRole {
    Unassigned,
    General,
    Backup,
}

impl DriveRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::General => "general",
            Self::Backup => "backup",
        }
    }

    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "unassigned" => Some(Self::Unassigned),
            "general" => Some(Self::General),
            "backup" => Some(Self::Backup),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScanStatus {
    pub job_id: String,
    pub root_path: String,
    pub tier: u8,
    pub status: String,
    #[ts(type = "number")]
    pub files_seen: i64,
    #[ts(type = "number")]
    pub files_indexed: i64,
    #[ts(type = "number")]
    pub files_skipped_perm: i64,
    #[ts(type = "number")]
    pub files_skipped_cloud: i64,
    pub coverage_pct: f64,
    #[ts(type = "number")]
    pub started_at: i64,
    #[ts(type = "number | null")]
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoverageErrorItem {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CoverageReport {
    pub job_id: String,
    pub root_path: String,
    #[ts(type = "number")]
    pub total_seen: i64,
    #[ts(type = "number")]
    pub indexed: i64,
    #[ts(type = "number")]
    pub skipped_perm: i64,
    #[ts(type = "number")]
    pub skipped_cloud: i64,
    pub coverage_pct: f64,
    pub errors: Vec<CoverageErrorItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct IndexStats {
    #[ts(type = "number")]
    pub total_files: i64,
    #[ts(type = "number")]
    pub active_files: i64,
    #[ts(type = "number")]
    pub quarantined_files: i64,
    #[ts(type = "number")]
    pub placeholder_files: i64,
    #[ts(type = "number")]
    pub total_chunks: i64,
    #[ts(type = "number")]
    pub fts_chunks: i64,
    #[ts(type = "number")]
    pub atomic_units: i64,
    #[ts(type = "number")]
    pub cache_dirs: i64,
    #[ts(type = "number")]
    pub pending_outbox: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchFilters {
    pub project_id: Option<String>,
    pub ext: Option<String>,
    #[ts(type = "number | null")]
    pub after_ts: Option<i64>,
    #[ts(type = "number | null")]
    pub before_ts: Option<i64>,
    pub sensitivity: Option<String>,
    pub include_quarantined: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    pub file_id: String,
    pub path: String,
    pub name: String,
    pub state: String,
    pub sensitivity: String,
    pub cloud_state: String,
    pub snippet: String,
    pub score: f64,
    pub match_reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchPage {
    pub query: String,
    #[ts(type = "number")]
    pub total_hits: usize,
    pub hits: Vec<SearchHit>,
    #[ts(type = "number")]
    pub took_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FileDetail {
    pub id: String,
    pub volume_id: String,
    pub path: String,
    pub name: String,
    pub ext: Option<String>,
    #[ts(type = "number")]
    pub size: i64,
    #[ts(type = "number")]
    pub mtime: i64,
    #[ts(type = "number")]
    pub nlink: i64,
    pub state: String,
    pub cloud_state: String,
    pub sensitivity: String,
    pub atomic_unit_id: Option<String>,
    pub unit_split_override: bool,
    pub chunks_preview: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AtomicUnit {
    pub id: String,
    pub volume_id: String,
    pub root_path: String,
    pub unit_type: String,
    pub marker_file: String,
    pub project_id: Option<String>,
    pub allow_split_override: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CacheDirEntry {
    pub id: String,
    pub volume_id: String,
    pub atomic_unit_id: Option<String>,
    pub path: String,
    pub cache_kind: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub file_count: i64,
    #[ts(type = "number | null")]
    pub source_max_mtime: Option<i64>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProposedActionInput {
    pub agent: Option<String>,
    pub action_type: ActionType,
    pub target_kind: TargetKind,
    pub file_id: Option<String>,
    pub atomic_unit_id: Option<String>,
    pub from_path: String,
    pub to_path: Option<String>,
    pub reason: String,
    pub confidence: f64,
    pub evidence_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RefilePlan {
    pub origin: String,
    pub actions: Vec<ProposedActionInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActionItem {
    pub id: String,
    pub batch_id: String,
    pub agent: Option<String>,
    pub action_type: ActionType,
    pub target_kind: TargetKind,
    pub file_id: Option<String>,
    pub atomic_unit_id: Option<String>,
    pub from_path: String,
    pub to_path: Option<String>,
    pub reason: String,
    pub confidence: f64,
    pub status: ActionStatus,
    pub rollback_conflict: Option<RollbackConflictKind>,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SimulationItem {
    pub action_id: String,
    pub from_path: String,
    pub to_path: Option<String>,
    pub effective_to_path: Option<String>,
    pub collision_renamed: bool,
    pub confidence: f64,
    pub reason: String,
    pub policy_verdict: String,
    pub policy_allowed: bool,
    pub atomic_unit_protected: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Simulation {
    pub batch_id: String,
    pub items: Vec<SimulationItem>,
    #[ts(type = "number")]
    pub blocked_count: usize,
    #[ts(type = "number")]
    pub review_count: usize,
    #[ts(type = "number")]
    pub ready_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExecuteResult {
    pub batch_id: String,
    pub status: BatchStatus,
    #[ts(type = "number")]
    pub executed_count: usize,
    #[ts(type = "number")]
    pub failed_count: usize,
    #[ts(type = "number")]
    pub skipped_count: usize,
    pub actions: Vec<ActionItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RollbackConflictItem {
    pub action_id: String,
    pub from_path: String,
    pub to_path: Option<String>,
    pub conflict: RollbackConflictKind,
    pub detail: String,
    pub recommended_choice: ConflictResolutionChoice,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RollbackResult {
    pub batch_id: String,
    pub status: BatchStatus,
    #[ts(type = "number")]
    pub reverted_count: usize,
    pub conflicts: Vec<RollbackConflictItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuarantineEntry {
    pub id: String,
    pub action_id: String,
    pub target_kind: TargetKind,
    pub file_id: Option<String>,
    pub atomic_unit_id: Option<String>,
    pub volume_id: String,
    pub original_path: String,
    pub quarantine_path: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub quarantined_at: i64,
    #[ts(type = "number")]
    pub expires_at: i64,
    #[ts(type = "number | null")]
    pub restored_at: Option<i64>,
    #[ts(type = "number | null")]
    pub purged_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QuarantineSpaceSummary {
    pub volume_id: String,
    #[ts(type = "number")]
    pub staged_bytes: u64,
    #[ts(type = "number")]
    pub staged_count: usize,
    #[ts(type = "number")]
    pub freed_bytes: u64,
    #[ts(type = "number")]
    pub purged_count: usize,
    #[ts(type = "number")]
    pub volume_total_bytes: u64,
    #[ts(type = "number")]
    pub volume_free_bytes: u64,
    pub soft_quota_pct: f64,
    pub quota_warning: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DupeCluster {
    pub cluster_key: String,
    pub keeper_file_id: String,
    pub member_file_ids: Vec<String>,
    #[ts(type = "number")]
    pub total_bytes: u64,
    #[ts(type = "number")]
    pub reclaimable_bytes: u64,
    pub hardlink_adjusted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DupeReport {
    pub group_by: String,
    pub clusters: Vec<DupeCluster>,
    #[ts(type = "number")]
    pub total_reclaimable_bytes: u64,
}

/// One place a duplicate lives: a folder (folder groups, batched file groups) or a file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DupeLocation {
    pub file_id: Option<String>,
    pub volume_id: String,
    pub volume_label: Option<String>,
    pub online: bool,
    pub path: String,
    pub is_backup: bool,
    #[ts(type = "number")]
    pub last_seen: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DupeGroupKind {
    /// Whole folders with identical contents.
    Folder,
    /// Folders sharing at least 90% of their files (both ways).
    PartialFolder,
    /// Individual files, batched by the folders they sit in.
    Files,
}

/// One duplicated file inside a group, attributed to one of the group's locations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DupeMember {
    pub location_index: usize,
    pub file_id: String,
    /// Path relative to its location folder.
    pub rel_path: String,
    pub blake3_hex: String,
    #[ts(type = "number")]
    pub size: i64,
}

/// One duplicate question (spec §7.5): a folder, near-identical folder, or batch of files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DupeGroup {
    /// Stable across rescans and drive-letter changes.
    pub group_id: String,
    pub kind: DupeGroupKind,
    /// Bytes of one full copy of the group's matched content.
    #[ts(type = "number")]
    pub size_bytes: i64,
    #[ts(type = "number")]
    pub file_count: i64,
    pub locations: Vec<DupeLocation>,
    pub keeper_index: usize,
    /// Relative paths present on only one side (partial folders only).
    pub differences: Vec<String>,
    pub needs_question: bool,
    pub members: Vec<DupeMember>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GroupedDupeReport {
    pub groups: Vec<DupeGroup>,
    #[ts(type = "number")]
    pub reclaimable_bytes: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DiskRescueReport {
    pub exact_dupe_clusters: Vec<DupeCluster>,
    pub cache_dirs: Vec<CacheDirEntry>,
    pub stale_oversized_files: Vec<FileDetail>,
    pub space_summary: Vec<QuarantineSpaceSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HealthDimension {
    pub id: String,
    pub label: String,
    pub score: f64,
    #[ts(type = "number")]
    pub offenders_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HealthBreakdown {
    pub overall_score: f64,
    pub dimensions: Vec<HealthDimension>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AuditEntry {
    #[ts(type = "number")]
    pub seq: i64,
    pub id: String,
    #[ts(type = "number")]
    pub ts: i64,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DoctorCheckResult {
    pub check_id: u8,
    pub name: String,
    pub passed: bool,
    pub details: String,
    pub repaired: bool,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DoctorReport {
    pub healthy: bool,
    #[ts(type = "number")]
    pub checked_at: i64,
    pub checks: Vec<DoctorCheckResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "event", content = "payload")]
#[ts(export)]
pub enum AegisEvent {
    ScanProgress {
        v: u8,
        job_id: String,
        #[ts(type = "number")]
        files_seen: i64,
        #[ts(type = "number")]
        files_indexed: i64,
        coverage_pct: f64,
    },
    IndexUpdated {
        v: u8,
        root_path: String,
        #[ts(type = "number")]
        files_changed: usize,
    },
    ActionProposed {
        v: u8,
        batch_id: String,
        #[ts(type = "number")]
        count: usize,
    },
    FindingCreated {
        v: u8,
        finding_id: String,
        base_severity: String,
    },
    HealthUpdated {
        v: u8,
        overall_score: f64,
    },
    /// A drive was plugged in. `known` = Aegis has seen it before.
    DriveArrived {
        v: u8,
        volume_id: String,
        label: Option<String>,
        #[ts(type = "number")]
        last_seen: i64,
        known: bool,
        role: String,
    },
    DriveDeparted {
        v: u8,
        volume_id: String,
    },
    DriveRefreshed {
        v: u8,
        report: crate::drives::RefreshReport,
    },
    DriveCatalogued {
        v: u8,
        volume_id: String,
        #[ts(type = "number")]
        files: i64,
    },
    /// Live count while a drive is being catalogued.
    DriveCatalogProgress {
        v: u8,
        volume_id: String,
        #[ts(type = "number")]
        files_seen: i64,
    },
    TopUpProgress {
        v: u8,
        backup_volume_id: String,
        #[ts(type = "number")]
        bytes_done: i64,
        #[ts(type = "number")]
        bytes_total: i64,
    },
    TopUpFinished {
        v: u8,
        backup_volume_id: String,
        result: crate::backup::topup::TopUpResult,
    },
    GuardHalted {
        v: u8,
        backup_volume_id: String,
        halts: Vec<crate::backup::guard::GuardHalt>,
    },
    /// A background drive job failed; `message` is owner-facing.
    JobFailed {
        v: u8,
        job: String,
        message: String,
    },
}

/// Generates TypeScript bindings and barrel file `index.ts` into `out_dir`.
pub fn write_ts_bindings(out_dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(out_dir)?;
    let cfg = ts_rs::Config::new()
        .with_out_dir(out_dir)
        .with_large_int("number");
    <IpcError as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <TargetKind as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <ActionType as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <ActionStatus as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <BatchStatus as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <RollbackConflictKind as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <ConflictResolutionChoice as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <LifecycleState as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <HardwareProfile as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <AutonomyDial as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <ActionClass as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <RootConfig as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <VolumeInfo as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DriveRole as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <ScanStatus as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <CoverageReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <IndexStats as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <SearchFilters as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <SearchPage as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <FileDetail as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <AtomicUnit as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <CacheDirEntry as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <RefilePlan as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <Simulation as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <ExecuteResult as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <RollbackResult as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <QuarantineEntry as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <QuarantineSpaceSummary as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DupeReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DupeLocation as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DupeGroupKind as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DupeMember as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DupeGroup as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <GroupedDupeReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::dupes::decisions::DupeChoice as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::dupes::decisions::DecisionOutcome as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::dupes::decisions::DupeDecisionItem as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::plan::DrivePlanEntry as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::plan::DrivePlan as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::plan::AppliedPlan as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::topup::TopUpKind as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::topup::TopUpAuthority as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::topup::TopUpItem as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::topup::TopUpPlan as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::topup::TopUpResult as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::topup::PreviousVersionItem as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::guard::GuardHalt as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::priority::TierShortfall as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::priority::TierCoverage as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::priority::BackupCoverage as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::backup::priority::BackupSourceItem as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::engine::Level as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::engine::Screen as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::engine::FindingAction as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::engine::Finding as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::engine::reminders::Reminder as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::engine::reminders::TrayState as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::drives::RefreshReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::drives::RefreshMode as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::drives::SyncReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::drives::CloneConflict as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <crate::fingerprint::FingerprintStats as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DiskRescueReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <HealthBreakdown as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <AuditEntry as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <DoctorReport as TS>::export_all(&cfg).map_err(std::io::Error::other)?;
    <AegisEvent as TS>::export_all(&cfg).map_err(std::io::Error::other)?;

    let mut stems = Vec::new();
    for entry in std::fs::read_dir(out_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("ts") {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
            if !stem.is_empty() && stem != "index" && stem != "client" {
                stems.push(stem.to_string());
            }
        }
    }
    stems.sort();
    let mut barrel = String::from("// Auto-generated by ts-rs from crates/aegis-core/src/contracts/mod.rs\n");
    for stem in stems {
        barrel.push_str(&format!("export * from './{stem}';\n"));
    }
    std::fs::write(out_dir.join("index.ts"), barrel)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_ts_bindings;
    use std::path::PathBuf;

    #[test]
    fn export_bindings() {
        let out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("src")
            .join("bindings");
        write_ts_bindings(&out_dir).expect("ts-rs export should succeed");
    }
}
