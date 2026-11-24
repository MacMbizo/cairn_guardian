import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Answer,
  AppliedPlan,
  Collection,
  BackupCoverage,
  BackupSourceItem,
  DecisionOutcome,
  DrivePlan,
  DriveRole,
  DupeChoice,
  BlueprintChoice,
  BlueprintSummary,
  DupeDecisionItem,
  EngineStatus,
  FilingRule,
  Placement,
  PlanOutcome,
  ProjectPlan,
  RecentBatch,
  ReunitePlan,
  RunReport,
  Scope,
  ScopeStatus,
  SearchFacets,
  GroupedDupeReport,
  PreviousVersionItem,
  RecogniseReport,
  Reminder,
  Screen,
  WeeklyNote,
  RefreshReport,
  SyncReport,
  TopUpAuthority,
  TopUpPlan,
} from "./index";
import type {
  ActionItem,
  AegisEvent,
  ActionStatus,
  AtomicUnit,
  AuditEntry,
  CleanupCandidate,
  CleanupScanReport,
  ConflictResolutionChoice,
  CoverageReport,
  DiskRescueReport,
  DiskTrendsReport,
  DoctorReport,
  DriveHealthItem,
  DupeReport,
  EmbedStatus,
  EmbedSyncReport,
  ExecuteResult,
  FileDetail,
  HardwareProfile,
  AssetRecoverItem,
  AssetScanOptions,
  ForgottenAssetsReport,
  HealthBreakdown,
  IndexStats,
  InstalledApp,
  KnowledgeCommitResult,
  KnowledgeDashboard,
  KnowledgeExtractOptions,
  KnowledgeExtractReport,
  KnowledgeQueryOptions,
  LeftoverFolder,
  LifecycleState,
  LibrarianOptions,
  LlmStatus,
  ProposalEditInput,
  QuarantineEntry,
  QuarantineSpaceSummary,
  RagAnswer,
  RagAskOptions,
  RecoveryKeyBundle,
  RefilePlan,
  ResearchBrief,
  ResearchCompanionReport,
  ResearchDocShelf,
  RollbackResult,
  RootConfig,
  ScanStatus,
  SearchFilters,
  SearchPage,
  Simulation,
  StartupEntry,
  VaultStatus,
  VersionDiffReport,
  VersionDiffSummary,
  VersionScanOptions,
  VersionScanReport,
  VolumeInfo,
} from "./index";

export * from "./index";

export function isTauriRuntime(): boolean {
  return (
    typeof window !== "undefined" &&
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)
  );
}

function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauriRuntime()) {
    return Promise.reject(
      new Error("Tauri desktop runtime is not active in browser-only preview.")
    );
  }
  return invoke<T>(cmd, args);
}

/** Channel the backend emits `AegisEvent`s on (scan progress, index updates). */
export const AEGIS_EVENT = "aegis://event";

export function onAegisEvent(cb: (event: AegisEvent) => void): Promise<UnlistenFn> {
  if (!isTauriRuntime()) return Promise.resolve(() => {});
  return listen<AegisEvent>(AEGIS_EVENT, (e) => cb(e.payload));
}

/** Extracts the human-readable message from an `IpcError` or any thrown value. */
export function errorMessage(err: unknown): string {
  if (err && typeof err === "object" && "message" in err) {
    return String((err as { message: unknown }).message);
  }
  return String(err);
}

export const aegis = {
  roots: {
    list: () => call<RootConfig[]>("roots_list"),
    add: (path: string) => call<RootConfig>("roots_add", { path }),
    remove: (path: string) => call<void>("roots_remove", { path }),
    toggle: (path: string, enabled: boolean) =>
      call<void>("roots_toggle", { path, enabled }),
  },
  ignore: {
    get: (root?: string) => call<string>("ignore_get", { root }),
    set: (content: string, root?: string) =>
      call<void>("ignore_set", { content, root }),
  },
  scan: {
    start: (roots?: string[], tiers?: number[]) =>
      call<ScanStatus[]>("scan_start", { roots, tiers }),
    status: (job?: string) => call<ScanStatus | null>("scan_status", { job }),
    coverageReport: (job?: string) =>
      call<CoverageReport>("scan_coverage_report", { job }),
  },
  volumes: {
    list: () => call<VolumeInfo[]>("volumes_list"),
  },
  index: {
    stats: () => call<IndexStats>("index_stats"),
  },
  search: {
    query: (q: string, filters?: SearchFilters, limit?: number) =>
      call<SearchPage>("search_query", { q, filters, limit }),
    /** What search can be narrowed by (categories, organisations, years, kinds, drives). */
    facets: () => call<SearchFacets>("search_facets"),
  },
  file: {
    get: (id: string) => call<FileDetail>("file_get", { id }),
    setLifecycle: (id: string, lifecycle: LifecycleState) =>
      call<void>("file_set_lifecycle", { id, lifecycle }),
    setSensitivity: (id: string, sensitivity: string) =>
      call<void>("file_set_sensitivity", { id, sensitivity }),
  },
  atomicUnit: {
    list: (volumeId?: string) =>
      call<AtomicUnit[]>("atomic_unit_list", { volumeId }),
    override: (
      target: { unitId?: string; fileId?: string },
      allowSplit: boolean
    ) =>
      call<void>("atomic_unit_override", {
        unitId: target.unitId,
        fileId: target.fileId,
        allowSplit,
      }),
  },
  dedupe: {
    report: () => call<DupeReport>("dedupe_report"),
    proposeQuarantine: (keeperFileId: string, memberFileIds: string[]) =>
      call<string>("dedupe_propose_quarantine", { keeperFileId, memberFileIds }),
  },
  actions: {
    propose: (plan: RefilePlan) => call<string>("actions_propose", { plan }),
    simulate: (batch: string) => call<Simulation>("actions_simulate", { batch }),
    approve: (batch: string, selection?: string[]) =>
      call<void>("actions_approve", { batch, selection }),
    execute: (batch: string) => call<ExecuteResult>("actions_execute", { batch }),
    reject: (batch: string, reason?: string) =>
      call<void>("actions_reject", { batch, reason }),
    rollback: (batch: string) => call<RollbackResult>("actions_rollback", { batch }),
    resolveConflict: (actionId: string, choice: ConflictResolutionChoice) =>
      call<RollbackResult>("actions_resolve_conflict", { actionId, choice }),
    list: (status?: ActionStatus) =>
      call<ActionItem[]>("actions_list", { status }),
  },
  quarantine: {
    list: (volumeId?: string) =>
      call<QuarantineEntry[]>("quarantine_list", { volumeId }),
    spaceSummary: (volumeId?: string) =>
      call<QuarantineSpaceSummary[]>("quarantine_space_summary", { volumeId }),
    restore: (entryIds: string[]) =>
      call<string>("quarantine_restore", { entryIds }),
    purge: (entryIds: string[], confirmedPermanent: boolean) =>
      call<string>("quarantine_purge", { entryIds, confirmedPermanent }),
  },
  health: {
    get: () => call<HealthBreakdown>("health_get"),
  },
  playbook: {
    diskRescue: () => call<DiskRescueReport>("playbook_disk_rescue"),
  },
  audit: {
    list: (limit?: number, offset?: number) =>
      call<AuditEntry[]>("audit_list", { limit, offset }),
  },
  hw: {
    profile: () => call<HardwareProfile>("hw_profile"),
  },
  doctor: {
    run: (repair?: boolean) => call<DoctorReport>("doctor_run", { repair }),
  },
  drives: {
    list: () => call<VolumeInfo[]>("drives_list"),
    sync: () => call<SyncReport>("drives_sync"),
    /** Starts a background catalog; completion arrives as a `DriveCatalogued` event. */
    catalog: (volumeId: string) => call<void>("drive_catalog", { volumeId }),
    /** Ids of drives whose catalog is running now (a repeat `catalog` call for them does nothing). */
    cataloguing: () => call<string[]>("drives_cataloguing"),
    refresh: (volumeId: string) => call<RefreshReport>("drive_refresh", { volumeId }),
    plan: () => call<DrivePlan>("drive_plan_get"),
    applyPlan: (roles: [string, DriveRole][]) =>
      call<AppliedPlan>("drive_plan_apply", { roles }),
  },
  dupes: {
    grouped: () => call<GroupedDupeReport>("dupes_grouped"),
    decide: (groupId: string, choice: DupeChoice) =>
      call<DecisionOutcome>("dupe_decide", { groupId, choice }),
    decisions: () => call<DupeDecisionItem[]>("dupe_decisions_list"),
    undoDecision: (id: string) => call<void>("dupe_decision_undo", { id }),
  },
  backup: {
    coverage: () => call<BackupCoverage | null>("backup_coverage"),
    sources: () => call<BackupSourceItem[]>("backup_sources_list"),
    addSource: (path: string, pinned: boolean) =>
      call<string>("backup_source_add", { path, pinned }),
    /** Returns the bytes that will stop being backed up (kept 90 days). */
    removeSource: (id: string) => call<number>("backup_source_remove", { id }),
    plan: () => call<TopUpPlan>("backup_topup_plan"),
    /** Starts a top-up in the background (`TopUpProgress` / `TopUpFinished` events). */
    start: (authority: TopUpAuthority) => call<void>("backup_topup_start", { authority }),
    pause: () => call<void>("backup_topup_pause"),
    overrideGuard: (folder: string) => call<void>("backup_guard_override", { folder }),
    grantStanding: () => call<string>("backup_standing_grant"),
    standingStatus: () => call<boolean>("backup_standing_status"),
    previousVersions: () => call<PreviousVersionItem[]>("previous_versions_list"),
    restoreVersion: (id: string, to?: string) =>
      call<string>("previous_version_restore", { id, to }),
  },
  filing: {
    scopes: () => call<ScopeStatus[]>("filing_scopes"),
    /** Both structures for a scope: `[tidy, fresh]`. */
    blueprints: (scope: Scope) => call<[BlueprintSummary, BlueprintSummary]>("filing_blueprints", { scope }),
    choose: (scope: Scope, choice: BlueprintChoice) => call<void>("filing_choose", { scope, choice }),
    nextBatch: (scope: Scope) => call<string | null>("filing_next_batch", { scope }),
    /** Approves the ticked items (all when none given), runs them and records the result. */
    approve: (batch: string, selection?: string[]) => call<RunReport>("filing_approve", { batch, selection }),
    /** Undo a batch; files that changed since are listed as conflicts. */
    undo: (batch: string) => call<RollbackResult>("filing_undo", { batch }),
    /** Turn a batch down: not offered again, and a "no" for its rules. */
    skip: (batch: string) => call<void>("filing_skip", { batch }),
    toSort: (scope: Scope) => call<Placement[]>("filing_to_sort", { scope }),
    destinations: (scope: Scope, fileIds: string[]) =>
      call<Record<string, string[]>>("filing_destinations", { scope, fileIds }),
    place: (scope: Scope, fileId: string, destFolder: string) =>
      call<void>("filing_place", { scope, fileId, destFolder }),
    /** Filing batches that have run, newest first (manual and automatic). */
    recent: () => call<RecentBatch[]>("filing_recent"),
    rules: () => call<FilingRule[]>("filing_rules"),
    stopRule: (id: string) => call<void>("filing_rule_stop", { id }),
    deleteRule: (id: string) => call<void>("filing_rule_delete", { id }),
  },
  context: {
    /** Recognised projects, those found in more than one place first. */
    projects: () => call<Collection[]>("context_projects"),
    /** Whether Aegis is looking for projects right now. */
    recognising: () => call<boolean>("context_recognising"),
    /** Looks for projects again (one pass at a time). */
    recogniseNow: () => call<RecogniseReport>("context_recognise_now"),
    /** One of the owner's answers about a card, or about one of its folders. */
    answer: (collectionId: string, answer: Answer, locationId?: string) =>
      call<void>("context_answer", { collectionId, locationId, answerGiven: answer }),
    /** Every project's plans (one per drive it lives on). */
    plans: () => call<ProjectPlan[]>("context_plans"),
    /** Runs one project's plan on one drive: every step or none. Undo with filing.undo. */
    runPlan: (collectionId: string, volumeId: string, expectedDigest: string) =>
      call<PlanOutcome>("context_run_plan", { collectionId, volumeId, expectedDigest }),
    /** Files from earlier filing batches that belong back in a project, design set or photo subject. */
    reunitePlans: () => call<ReunitePlan[]>("context_reunite_plans"),
    /** Puts one group's scattered files back together after verifying its digest. */
    runReunite: (collectionId: string, volumeId: string, expectedDigest: string) =>
      call<PlanOutcome>("context_run_reunite", { collectionId, volumeId, expectedDigest }),
  },
  engine: {
    remindersOpen: () => call<Reminder[]>("reminders_open"),
    /** "Not now". Returns true when that kind of reminder moved to the weekly note. */
    dismiss: (id: string) => call<boolean>("reminder_dismiss", { id }),
    act: (id: string) => call<void>("reminder_act", { id }),
    undemote: (kind: string) => call<void>("reminder_undemote", { kind }),
    noteLatest: () => call<WeeklyNote | null>("note_latest"),
    noteMarkRead: (week: string) => call<void>("note_mark_read", { week }),
    pause: (mode: "hour" | "tomorrow" | "resume") => call<void>("engine_pause", { mode }),
    status: () => call<EngineStatus>("engine_status"),
    settingsSet: (s: { autostart?: boolean; noteDay?: number; noteHour?: number; shortcut?: string }) =>
      call<void>("engine_settings_set", s),
    /** The screen a notification or the tray asked for before this page loaded (taken once). */
    navTake: () => call<Screen | null>("nav_take"),
    tripwireWasMe: (folder: string) => call<void>("tripwire_was_me", { folder }),
    /** Folders whose backup the tripwire has locked. */
    tripwireLocks: () => call<string[]>("tripwire_locks"),
  },
  vault: {
    status: () => call<VaultStatus>("vault_status"),
    getInitialRecoveryKey: () =>
      call<RecoveryKeyBundle>("vault_get_initial_recovery_key"),
    confirmRecoveryBackup: (checks: [number, string][]) =>
      call<VaultStatus>("vault_confirm_recovery_backup", { checks }),
    revealRecoveryKey: () => call<RecoveryKeyBundle>("vault_reveal_recovery_key"),
    unlockWithRecovery: (input: string) =>
      call<VaultStatus>("vault_unlock_with_recovery", { input }),
    unlockSensitive: () => call<VaultStatus>("vault_unlock_sensitive"),
    lockSensitive: () => call<VaultStatus>("vault_lock_sensitive"),
  },
  embed: {
    status: () => call<EmbedStatus>("embed_status"),
    syncNow: (limit?: number) => call<EmbedSyncReport>("embed_sync_now", { limit }),
    selectModel: (modelPath: string | null) =>
      call<EmbedStatus>("embed_select_model", { modelPath }),
    unload: () => call<EmbedStatus>("embed_unload"),
  },
  rag: {
    status: () => call<LlmStatus>("rag_status"),
    selectModel: (modelPath: string | null) =>
      call<LlmStatus>("rag_select_model", { modelPath }),
    unload: () => call<LlmStatus>("rag_unload"),
    cancel: () => call<void>("rag_cancel"),
    ask: (question: string, options?: RagAskOptions) =>
      call<RagAnswer>("rag_ask", { question, options }),
  },
  assets: {
    scan: (options?: Partial<AssetScanOptions>) =>
      call<ForgottenAssetsReport>("assets_scan", {
        options: options
          ? {
              min_dormancy_days: options.min_dormancy_days ?? null,
              families: options.families ?? null,
              reasons: options.reasons ?? null,
              drive_status: options.drive_status ?? null,
              volume_id: options.volume_id ?? null,
              query: options.query ?? null,
              include_dismissed: options.include_dismissed ?? null,
              limit: options.limit ?? null,
            }
          : undefined,
      }),
    enrichMetadata: (fileIds: string[]) =>
      call<number>("assets_enrich_metadata", { fileIds }),
    dismiss: (fileId: string, reason?: string) =>
      call<void>("assets_dismiss", { fileId, reason }),
    undismiss: (fileId: string) =>
      call<void>("assets_undismiss", { fileId }),
    proposeRecovery: (items: AssetRecoverItem[]) =>
      call<string>("assets_propose_recovery", { items }),
  },
  versions: {
    scan: (options?: Partial<VersionScanOptions>) =>
      call<VersionScanReport>("versions_scan", {
        options: options
          ? {
              query: options.query ?? null,
              collection_id: options.collection_id ?? null,
              volume_id: options.volume_id ?? null,
              signal: options.signal ?? null,
              min_similarity: options.min_similarity ?? null,
              only_with_dupes: options.only_with_dupes ?? null,
              include_dismissed: options.include_dismissed ?? null,
              limit: options.limit ?? null,
            }
          : undefined,
      }),
    diff: (olderFileId: string, newerFileId: string) =>
      call<VersionDiffReport>("versions_diff", { olderFileId, newerFileId }),
    summarizeDiff: (olderFileId: string, newerFileId: string) =>
      call<VersionDiffSummary>("versions_summarize_diff", { olderFileId, newerFileId }),
    pinLatest: (chainKey: string, fileId: string) =>
      call<void>("versions_pin_latest", { chainKey, fileId }),
    dismiss: (chainKey: string) => call<void>("versions_dismiss", { chainKey }),
    undismiss: (chainKey: string) => call<void>("versions_undismiss", { chainKey }),
    proposeArchive: (fileIds: string[], destDir: string) =>
      call<string>("versions_propose_archive", { fileIds, destDir }),
    proposeQuarantineDupes: (keeperFileId: string, duplicateFileIds: string[]) =>
      call<string>("versions_propose_quarantine_dupes", {
        keeperFileId,
        duplicateFileIds,
      }),
  },
  knowledge: {
    dashboard: (options?: Partial<KnowledgeQueryOptions>) =>
      call<KnowledgeDashboard>("knowledge_dashboard", {
        options: options
          ? {
              query: options.query ?? null,
              entity_type: options.entity_type ?? null,
              status: options.status ?? null,
              limit: options.limit ?? null,
            }
          : undefined,
      }),
    extractScan: (options?: Partial<KnowledgeExtractOptions>) =>
      call<KnowledgeExtractReport>("knowledge_extract_scan", {
        options: options
          ? {
              file_ids: options.file_ids ?? null,
              collection_id: options.collection_id ?? null,
              include_sensitive: options.include_sensitive ?? null,
              force_rescan: options.force_rescan ?? null,
              limit_files: options.limit_files ?? null,
            }
          : undefined,
      }),
    extractFileLlm: (fileId: string) =>
      call<KnowledgeExtractReport>("knowledge_extract_file_llm", { fileId }),
    approveProposal: (proposalId: string, edit?: Partial<ProposalEditInput>) =>
      call<KnowledgeCommitResult>("knowledge_approve_proposal", {
        proposalId,
        edit: edit
          ? {
              name: edit.name ?? null,
              entity_type: edit.entity_type ?? null,
              summary: edit.summary ?? null,
              assignee: edit.assignee ?? null,
              due_date: edit.due_date ?? null,
              project_hint: edit.project_hint ?? null,
            }
          : undefined,
      }),
    approveAll: (proposalIds: string[]) =>
      call<KnowledgeCommitResult[]>("knowledge_approve_all", { proposalIds }),
    undoProposal: (proposalId: string) =>
      call<void>("knowledge_undo_proposal", { proposalId }),
    dismissProposal: (proposalId: string) =>
      call<void>("knowledge_dismiss_proposal", { proposalId }),
    reopenProposal: (proposalId: string) =>
      call<void>("knowledge_reopen_proposal", { proposalId }),
    setEntityStatus: (entityId: string, statusTag: string) =>
      call<void>("knowledge_set_entity_status", { entityId, statusTag }),
    setAudioOptIn: (enabled: boolean) =>
      call<boolean>("knowledge_set_audio_opt_in", { enabled }),
  },
  librarian: {
    recentDocs: (params?: {
      query?: string;
      collectionId?: string;
      limit?: number;
      includeSensitive?: boolean;
    }) =>
      call<ResearchDocShelf>("librarian_recent_docs", {
        query: params?.query ?? null,
        collectionId: params?.collectionId ?? null,
        limit: params?.limit ?? null,
        includeSensitive: params?.includeSensitive ?? null,
      }),
    relatedForFile: (fileId: string, options?: Partial<LibrarianOptions>) =>
      call<ResearchCompanionReport>("librarian_related_for_file", {
        fileId,
        options: options
          ? {
              limit: options.limit ?? null,
              min_score: options.min_score ?? null,
              cross_project_only: options.cross_project_only ?? null,
              semantic_only_filter: options.semantic_only_filter ?? null,
              include_dismissed: options.include_dismissed ?? null,
              include_sensitive: options.include_sensitive ?? null,
            }
          : undefined,
      }),
    relatedForTopic: (topic: string, options?: Partial<LibrarianOptions>) =>
      call<ResearchCompanionReport>("librarian_related_for_topic", {
        topic,
        options: options
          ? {
              limit: options.limit ?? null,
              min_score: options.min_score ?? null,
              cross_project_only: options.cross_project_only ?? null,
              semantic_only_filter: options.semantic_only_filter ?? null,
              include_dismissed: options.include_dismissed ?? null,
              include_sensitive: options.include_sensitive ?? null,
            }
          : undefined,
      }),
    synthesizeBrief: (params: {
      sourceFileId?: string | null;
      topic?: string | null;
      relatedFileIds: string[];
      includeSensitive?: boolean;
    }) =>
      call<ResearchBrief>("librarian_synthesize_brief", {
        sourceFileId: params.sourceFileId ?? null,
        topic: params.topic ?? null,
        relatedFileIds: params.relatedFileIds,
        includeSensitive: params.includeSensitive ?? null,
      }),
    pin: (sourceFileId: string, relatedFileId: string, note?: string) =>
      call<boolean>("librarian_pin", {
        sourceFileId,
        relatedFileId,
        note: note ?? null,
      }),
    unpin: (sourceFileId: string, relatedFileId: string) =>
      call<boolean>("librarian_unpin", { sourceFileId, relatedFileId }),
    dismiss: (sourceFileId: string, relatedFileId: string) =>
      call<boolean>("librarian_dismiss", { sourceFileId, relatedFileId }),
    undismiss: (sourceFileId: string, relatedFileId: string) =>
      call<boolean>("librarian_undismiss", { sourceFileId, relatedFileId }),
  },
  care: {
    scanCleanup: () => call<CleanupScanReport>("care_scan_cleanup"),
    proposeCleanup: (items: CleanupCandidate[]) =>
      call<string>("care_propose_cleanup", { items }),
    installedApps: () => call<InstalledApp[]>("care_installed_apps"),
    leftoverFolders: () => call<LeftoverFolder[]>("care_leftover_folders"),
    launchUninstaller: (appId: string) =>
      call<void>("care_launch_uninstaller", { appId }),
    openWindowsApps: () => call<void>("care_open_windows_apps"),
    startupItems: () => call<StartupEntry[]>("care_startup_items"),
    openStartupSettings: () => call<void>("care_open_startup_settings"),
    driveHealth: () => call<DriveHealthItem[]>("care_drive_health"),
    diskTrends: (days?: number) => call<DiskTrendsReport>("care_disk_trends", { days }),
  },
};
