import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ActionItem,
  AegisEvent,
  ActionStatus,
  AtomicUnit,
  AuditEntry,
  ConflictResolutionChoice,
  CoverageReport,
  DiskRescueReport,
  DoctorReport,
  DupeReport,
  ExecuteResult,
  FileDetail,
  HardwareProfile,
  HealthBreakdown,
  IndexStats,
  LifecycleState,
  QuarantineEntry,
  QuarantineSpaceSummary,
  RefilePlan,
  RollbackResult,
  RootConfig,
  ScanStatus,
  SearchFilters,
  SearchPage,
  Simulation,
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
  },
  file: {
    get: (id: string) => call<FileDetail>("file_get", { id }),
    setLifecycle: (id: string, lifecycle: LifecycleState) =>
      call<void>("file_set_lifecycle", { id, lifecycle }),
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
};
