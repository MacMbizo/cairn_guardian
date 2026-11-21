import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  AlertTriangle,
  Archive,
  CheckCircle2,
  Copy,
  CopyCheck,
  Cpu,
  FolderGit2,
  FolderOpen,
  HardDrive,
  Lock,
  Play,
  RefreshCw,
  RotateCcw,
  Search,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Stethoscope,
  Trash2,
  Usb,
  XCircle,
} from "lucide-react";
import { HomeTiles } from "./components/HomeTiles";
import { PlugBanner } from "./components/PlugBanner";
import { DrivesScreen } from "./screens/Drives";
import { DupeReview } from "./screens/DupeReview";
import { useDrives } from "./screens/useDrives";
import {
  aegis,
  errorMessage,
  isTauriRuntime,
  onAegisEvent,
  type ActionItem,
  type CacheDirEntry,
  type DupeCluster,
  type ConflictResolutionChoice,
  type CoverageReport,
  type DiskRescueReport,
  type DoctorReport,
  type HardwareProfile,
  type HealthBreakdown,
  type IndexStats,
  type QuarantineEntry,
  type QuarantineSpaceSummary,
  type RollbackConflictItem,
  type RollbackResult,
  type RootConfig,
  type ScanStatus,
  type SearchPage,
  type Simulation,
} from "./bindings/client";

type TabId = "dashboard" | "drives" | "dupes" | "search" | "rescue" | "simulator" | "doctor";

function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const idx = Math.min(
    units.length - 1,
    Math.floor(Math.log(bytes) / Math.log(1024))
  );
  const val = bytes / 1024 ** idx;
  return `${val.toFixed(idx === 0 ? 0 : 1)} ${units[idx]}`;
}

export default function App() {
  const desktop = useMemo(() => isTauriRuntime(), []);
  const [tab, setTab] = useState<TabId>("dashboard");
  const drivesData = useDrives();
  const [statusMsg, setStatusMsg] = useState<string>(
    desktop
      ? "Aegis Guardian core ready."
      : "Browser preview mode — launch via Tauri desktop runtime for local filesystem IPC."
  );
  const [busy, setBusy] = useState(false);

  const [hwProfile, setHwProfile] = useState<HardwareProfile>("standard");
  const [roots, setRoots] = useState<RootConfig[]>([]);
  const [newRootPath, setNewRootPath] = useState("");
  const [indexStats, setIndexStats] = useState<IndexStats | null>(null);
  const [scanStatus, setScanStatus] = useState<ScanStatus | null>(null);
  const [coverageModal, setCoverageModal] = useState<CoverageReport | null>(null);
  const [health, setHealth] = useState<HealthBreakdown | null>(null);
  const [spaceSummaries, setSpaceSummaries] = useState<QuarantineSpaceSummary[]>([]);

  // Search
  const [query, setQuery] = useState("");
  const [includeQuarantined, setIncludeQuarantined] = useState(false);
  const [searchPage, setSearchPage] = useState<SearchPage | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);

  // Disk Rescue & Quarantine
  const [rescue, setRescue] = useState<DiskRescueReport | null>(null);
  const [quarantineEntries, setQuarantineEntries] = useState<QuarantineEntry[]>([]);
  const [purgeConfirmOpen, setPurgeConfirmOpen] = useState(false);

  // Simulator & Actions
  const [actions, setActions] = useState<ActionItem[]>([]);
  const [simulation, setSimulation] = useState<Simulation | null>(null);
  const [conflicts, setConflicts] = useState<RollbackConflictItem[]>([]);
  const [conflictChoices, setConflictChoices] = useState<
    Record<string, ConflictResolutionChoice>
  >({});

  // Per-batch selection of proposed actions for partial approval
  const [selected, setSelected] = useState<Record<string, string[]>>({});

  // Doctor
  const [doctorReport, setDoctorReport] = useState<DoctorReport | null>(null);

  const tabRef = useRef<TabId>(tab);
  tabRef.current = tab;

  // The Disk Rescue report runs the full duplicate analysis, so it is only loaded while
  // that tab is open instead of on every refresh.
  const refreshRescue = useCallback(async () => {
    if (!desktop) return;
    try {
      setRescue(await aegis.playbook.diskRescue());
    } catch (err) {
      setStatusMsg(`Could not load Disk Rescue: ${errorMessage(err)}`);
    }
  }, [desktop]);

  const refreshAll = useCallback(async () => {
    if (!desktop) return;
    try {
      const [prof, rts, stats, latestScan, h, spaces, act, qList] = await Promise.all([
        aegis.hw.profile(),
        aegis.roots.list(),
        aegis.index.stats(),
        aegis.scan.status(),
        aegis.health.get(),
        aegis.quarantine.spaceSummary(),
        aegis.actions.list(),
        aegis.quarantine.list(),
      ]);
      setHwProfile(prof);
      setRoots(rts);
      setIndexStats(stats);
      setScanStatus(latestScan);
      setHealth(h);
      setSpaceSummaries(spaces);
      setActions(act);
      setQuarantineEntries(qList);
      if (tabRef.current === "rescue") await refreshRescue();
    } catch (err) {
      setStatusMsg(`Error refreshing state: ${errorMessage(err)}`);
    }
  }, [desktop, refreshRescue]);

  useEffect(() => {
    void refreshAll();
  }, [refreshAll]);

  useEffect(() => {
    if (tab === "rescue") void refreshRescue();
  }, [tab, refreshRescue]);

  // Live scan progress from the backend (scans run off the UI thread).
  useEffect(() => {
    const unlisten = onAegisEvent((ev) => {
      if (ev.event === "ScanProgress") {
        setStatusMsg(
          `Scanning… ${ev.payload.files_seen.toLocaleString()} files seen, ${ev.payload.files_indexed.toLocaleString()} indexed`
        );
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  /** Runs one user action with a busy guard and a readable error in the status bar. */
  const run = useCallback(
    async (label: string, fn: () => Promise<string | void>, refresh = true) => {
      setBusy(true);
      try {
        const msg = await fn();
        if (msg) setStatusMsg(msg);
        if (refresh) await refreshAll();
      } catch (err) {
        setStatusMsg(`${label} failed: ${errorMessage(err)}`);
      } finally {
        setBusy(false);
      }
    },
    [refreshAll]
  );

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setTab("search");
        setTimeout(() => searchInputRef.current?.focus(), 0);
      }
      if (e.key === "Escape") {
        setCoverageModal(null);
        setPurgeConfirmOpen(false);
        closeConflictsRef.current();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const addRoot = (path: string) =>
    run("Add root", async () => {
      const added = await aegis.roots.add(path);
      setNewRootPath("");
      return `Added root ${added.path}`;
    });

  const handleAddRoot = (e: React.FormEvent) => {
    e.preventDefault();
    if (newRootPath.trim()) void addRoot(newRootPath.trim());
  };

  const handleBrowseRoot = async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked === "string" && picked) void addRoot(picked);
    } catch (err) {
      setStatusMsg(`Folder picker failed: ${errorMessage(err)}`);
    }
  };

  const handleStartScan = () =>
    run("Scan", async () => {
      setStatusMsg("Starting scan…");
      const results = await aegis.scan.start(undefined, [1, 2]);
      const seen = results.reduce((acc, s) => acc + s.files_seen, 0);
      return `Scan completed across ${results.length} root(s): ${seen.toLocaleString()} files seen.`;
    });

  const handleOpenCoverage = async () => {
    if (!scanStatus) return;
    try {
      const report = await aegis.scan.coverageReport(scanStatus.job_id);
      setCoverageModal(report);
    } catch (err) {
      setStatusMsg(`Could not load coverage report: ${errorMessage(err)}`);
    }
  };

  const handleSearch = (e: React.FormEvent) => {
    e.preventDefault();
    void run(
      "Search",
      async () => {
        const page = await aegis.search.query(
          query,
          {
            project_id: null,
            ext: null,
            after_ts: null,
            before_ts: null,
            sensitivity: null,
            include_quarantined: includeQuarantined,
          },
          30
        );
        setSearchPage(page);
        return `Search found ${page.total_hits} match(es) in ${page.took_ms}ms.`;
      },
      false
    );
  };

  /** Simulates a freshly proposed batch and opens it in the Simulator tab. */
  const openInSimulator = async (batchId: string) => {
    setSimulation(await aegis.actions.simulate(batchId));
    setTab("simulator");
  };

  const handleProposeDupes = (cluster: DupeCluster) =>
    run("Duplicate proposal", async () => {
      const batchId = await aegis.dedupe.proposeQuarantine(
        cluster.keeper_file_id,
        cluster.member_file_ids
      );
      await openInSimulator(batchId);
      return `Staged ${cluster.member_file_ids.length - 1} duplicate(s) for quarantine in batch ${batchId.slice(0, 8)}. Review, approve, then execute.`;
    });

  const handleStageCacheQuarantine = (cache: CacheDirEntry) =>
    run("Proposal", async () => {
      const batchId = await aegis.actions.propose({
        origin: "dedupe",
        actions: [
          {
            agent: "Housekeeper",
            action_type: "quarantine",
            target_kind: "dir",
            file_id: null,
            atomic_unit_id: cache.atomic_unit_id,
            from_path: cache.path,
            to_path: null,
            reason: `Reclaim ${formatBytes(cache.size_bytes)} dormant ${cache.cache_kind}`,
            confidence: 0.96,
            evidence_json: "[]",
          },
        ],
      });
      await openInSimulator(batchId);
      return `Staged cache quarantine batch ${batchId.slice(0, 8)} in Simulator. Review, approve, then execute.`;
    });

  const handlePurgeAllQuarantine = () =>
    run("Purge", async () => {
      const ids = quarantineEntries.map((q) => q.id);
      if (ids.length === 0) return;
      setPurgeConfirmOpen(false);
      await aegis.quarantine.purge(ids, true);
      return `Permanently purged ${ids.length} quarantined item(s). Disk space freed.`;
    });

  const handleRestore = (entry: QuarantineEntry) =>
    run("Restore", async () => {
      await aegis.quarantine.restore([entry.id]);
      return `Restored ${entry.original_path} from quarantine.`;
    });

  const handleSimulate = (batchId: string) =>
    run(
      "Simulate",
      async () => {
        setSimulation(await aegis.actions.simulate(batchId));
      },
      false
    );

  const handleApprove = (batchId: string, selection?: string[]) =>
    run("Approve", async () => {
      await aegis.actions.approve(batchId, selection);
      setSelected((prev) => ({ ...prev, [batchId]: [] }));
      return selection
        ? `Approved ${selection.length} selected action(s) in batch ${batchId.slice(0, 8)}.`
        : `Approved batch ${batchId.slice(0, 8)}.`;
    });

  const handleReject = (batchId: string) =>
    run("Reject", async () => {
      await aegis.actions.reject(batchId);
      return `Rejected pending actions in batch ${batchId.slice(0, 8)}.`;
    });

  const handleExecute = (batchId: string) =>
    run("Execute", async () => {
      const r = await aegis.actions.execute(batchId);
      return `Executed ${r.executed_count} action(s), ${r.failed_count} failed, ${r.skipped_count} skipped.`;
    });

  const describeRollback = (res: RollbackResult) =>
    res.status === "rolled_back"
      ? `Rollback complete: ${res.reverted_count} action(s) reverted in batch ${res.batch_id.slice(0, 8)}.`
      : `Rollback finished with ${res.reverted_count} action(s) reverted; some items were skipped or unrecoverable (batch is partial).`;

  const handleRollbackBatch = (batchId: string) =>
    run("Rollback", async () => {
      const res = await aegis.actions.rollback(batchId);
      if (res.conflicts.length > 0) {
        const defaults: Record<string, ConflictResolutionChoice> = {};
        for (const c of res.conflicts) {
          defaults[c.action_id] = c.recommended_choice;
        }
        setConflictChoices(defaults);
        setConflicts(res.conflicts);
        return `Rollback paused before touching disk: ${res.conflicts.length} conflict(s) need a decision.`;
      }
      return describeRollback(res);
    });

  const handleResolveConflict = (actionId: string) => {
    const choice = conflictChoices[actionId];
    if (!choice) return;
    void run("Conflict resolution", async () => {
      const res = await aegis.actions.resolveConflict(actionId, choice);
      setConflicts(res.conflicts);
      if (choice === "abort_batch") return "Rollback aborted; remaining actions stay executed.";
      if (res.conflicts.length > 0) {
        return `Applied ${choice.replace(/_/g, " ")}. ${res.conflicts.length} conflict(s) left; the rest of the batch reverts once they are resolved.`;
      }
      return describeRollback(res);
    });
  };

  const closeConflicts = () => {
    if (conflicts.length > 0) {
      setStatusMsg(
        "Rollback paused with unresolved conflicts. Click Rollback on the batch to resume."
      );
    }
    setConflicts([]);
  };

  const closeConflictsRef = useRef(closeConflicts);
  closeConflictsRef.current = closeConflicts;

  const handleRunDoctor = (repair: boolean) =>
    run("Doctor", async () => {
      const rep = await aegis.doctor.run(repair);
      setDoctorReport(rep);
      return rep.healthy
        ? "Doctor passed all 8 integrity checks."
        : "Doctor flagged integrity issues requiring attention.";
    });

  const totalStagedBytes = spaceSummaries.reduce((a, s) => a + s.staged_bytes, 0);
  const totalFreedBytes = spaceSummaries.reduce((a, s) => a + s.freed_bytes, 0);
  const anyQuotaWarning = spaceSummaries.some((s) => s.quota_warning);

  const groupedBatches = useMemo(() => {
    const map = new Map<string, ActionItem[]>();
    for (const act of actions) {
      const list = map.get(act.batch_id) ?? [];
      list.push(act);
      map.set(act.batch_id, list);
    }
    return Array.from(map.entries());
  }, [actions]);

  const navItems: { id: TabId; label: string; icon: typeof ShieldCheck }[] = [
    { id: "dashboard", label: "Home", icon: HardDrive },
    { id: "drives", label: "Drives & Backup", icon: Usb },
    { id: "dupes", label: "Duplicates", icon: CopyCheck },
    { id: "search", label: "Keyword Search (Ctrl+K)", icon: Search },
    { id: "rescue", label: "Disk Rescue & Quarantine", icon: Archive },
    { id: "simulator", label: "Simulator & Actions", icon: Sparkles },
    { id: "doctor", label: "Aegis Doctor", icon: Stethoscope },
  ];

  return (
    <div className="flex h-screen bg-slate-950 text-slate-100">
      {/* Left Navigation Rail */}
      <aside
        aria-label="Primary Navigation"
        className="flex w-64 flex-col border-r border-slate-800 bg-slate-900/70 p-4"
      >
        <div className="mb-6 flex items-center gap-2.5 px-2">
          <ShieldCheck className="h-6 w-6 text-emerald-400" aria-hidden="true" />
          <div>
            <h1 className="text-sm font-semibold tracking-wide text-slate-50">
              AEGIS GUARDIAN
            </h1>
            <p className="text-xs text-slate-400">Local Digital Twin v0.1</p>
          </div>
        </div>

        <nav className="flex-1 space-y-1">
          {navItems.map((item) => {
            const Icon = item.icon;
            const active = tab === item.id;
            return (
              <button
                key={item.id}
                type="button"
                onClick={() => setTab(item.id)}
                className={`flex w-full items-center gap-2.5 rounded-lg px-3 py-2 text-left text-sm font-medium transition focus:outline-none focus:ring-2 focus:ring-emerald-400 ${
                  active
                    ? "bg-emerald-500/15 text-emerald-300"
                    : "text-slate-300 hover:bg-slate-800/70"
                }`}
              >
                <Icon className="h-4 w-4 shrink-0" aria-hidden="true" />
                <span>{item.label}</span>
              </button>
            );
          })}
        </nav>

        <div className="mt-4 rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs">
          <div className="flex items-center justify-between text-slate-300">
            <span className="flex items-center gap-1.5">
              <Cpu className="h-3.5 w-3.5 text-emerald-400" aria-hidden="true" />
              Hardware Profile
            </span>
            <span className="rounded bg-slate-800 px-1.5 py-0.5 font-semibold uppercase text-emerald-300">
              {hwProfile}
            </span>
          </div>
          <p className="mt-1.5 text-slate-400">
            Autonomy: <strong className="text-slate-200">Suggest only</strong>
            <span className="block">Backups run by themselves once you allow it.</span>
          </p>
        </div>
      </aside>

      {/* Main Content */}
      <main className="flex flex-1 flex-col overflow-hidden">
        <header className="flex items-center justify-between border-b border-slate-800 bg-slate-900/40 px-6 py-3">
          <p
            aria-live="polite"
            className="flex items-center gap-2 text-xs text-slate-300"
          >
            {busy && <RefreshCw className="h-3.5 w-3.5 animate-spin text-emerald-400" />}
            <span>{statusMsg}</span>
          </p>
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={!desktop || busy}
              onClick={() => void refreshAll()}
              className="rounded-md border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40"
            >
              Refresh
            </button>
            <button
              type="button"
              disabled={!desktop || busy || roots.length === 0}
              onClick={() => void handleStartScan()}
              className="flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
            >
              <Play className="h-3.5 w-3.5" aria-hidden="true" />
              Scan Roots
            </button>
          </div>
        </header>
        <PlugBanner onOpen={(t) => { setTab(t); void drivesData.reload(); }} />

        <div className="flex-1 overflow-y-auto p-6">
          {tab === "drives" && (
            <DrivesScreen data={drivesData} onReviewBatches={() => { setTab("simulator"); void refreshAll(); }} />
          )}
          {tab === "dupes" && (
            <DupeReview data={drivesData} onReviewBatches={() => { setTab("simulator"); void refreshAll(); }} />
          )}
          {tab === "dashboard" && (
            <div className="space-y-6">
              <HomeTiles data={drivesData} onOpen={setTab} />
              {/* Honest Coverage Banner (D3 §2) */}
              {scanStatus && (
                <section
                  aria-label="Index Coverage"
                  className={`rounded-xl border p-4 ${
                    scanStatus.files_skipped_perm > 0 ||
                    scanStatus.files_skipped_cloud > 0
                      ? "border-amber-500/40 bg-amber-950/20"
                      : "border-emerald-500/30 bg-emerald-950/20"
                  }`}
                >
                  <div className="flex flex-wrap items-center justify-between gap-3">
                    <div className="flex items-center gap-2.5">
                      {scanStatus.files_skipped_perm > 0 ||
                      scanStatus.files_skipped_cloud > 0 ? (
                        <AlertTriangle
                          className="h-5 w-5 text-amber-400"
                          aria-hidden="true"
                        />
                      ) : (
                        <CheckCircle2
                          className="h-5 w-5 text-emerald-400"
                          aria-hidden="true"
                        />
                      )}
                      <div>
                        <h2 className="text-sm font-semibold">
                          Indexed {scanStatus.coverage_pct.toFixed(1)}% of{" "}
                          {scanStatus.files_seen} files
                        </h2>
                        <p className="text-xs text-slate-300">
                          {scanStatus.files_skipped_perm} permission-skipped ·{" "}
                          {scanStatus.files_skipped_cloud} cloud placeholders (never
                          hydrated)
                        </p>
                      </div>
                    </div>
                    <button
                      type="button"
                      onClick={() => void handleOpenCoverage()}
                      className="rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium hover:bg-slate-800"
                    >
                      View Coverage Report
                    </button>
                  </div>
                </section>
              )}

              {/* Metrics Grid */}
              <div className="grid grid-cols-1 gap-4 md:grid-cols-3">
                <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                  <div className="text-xs text-slate-400">Digital Twin Index</div>
                  <div className="mt-1 text-2xl font-bold">
                    {indexStats?.active_files ?? 0}{" "}
                    <span className="text-sm font-normal text-slate-400">
                      active files
                    </span>
                  </div>
                  <div className="mt-2 text-xs text-slate-400">
                    {indexStats?.total_chunks ?? 0} chunks ·{" "}
                    {indexStats?.atomic_units ?? 0} atomic units ·{" "}
                    {indexStats?.cache_dirs ?? 0} cache dirs
                  </div>
                </div>

                {/* Quarantine Space Clarity (Promoted V1.5-01) */}
                <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                  <div className="flex items-center justify-between text-xs text-slate-400">
                    <span>Quarantine vs. Freed Space</span>
                    {anyQuotaWarning && (
                      <span className="flex items-center gap-1 text-amber-400">
                        <AlertTriangle className="h-3.5 w-3.5" /> Quota ≥10%
                      </span>
                    )}
                  </div>
                  <div className="mt-1 text-lg font-bold text-amber-300">
                    {formatBytes(totalStagedBytes)}{" "}
                    <span className="text-xs font-normal text-slate-400">
                      Staged in Quarantine (30d reversible)
                    </span>
                  </div>
                  <div className="mt-1 text-sm font-semibold text-emerald-400">
                    {formatBytes(totalFreedBytes)}{" "}
                    <span className="text-xs font-normal text-slate-400">
                      Actually Freed on Disk
                    </span>
                  </div>
                </div>

                <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                  <div className="text-xs text-slate-400">Environment Health</div>
                  <div className="mt-1 text-2xl font-bold text-emerald-400">
                    {health ? health.overall_score.toFixed(1) : "—"}
                    <span className="text-sm font-normal text-slate-400"> / 100</span>
                  </div>
                  <div className="mt-2 space-y-1">
                    {health?.dimensions.slice(0, 3).map((d) => (
                      <div
                        key={d.id}
                        className="flex items-center justify-between text-xs text-slate-300"
                      >
                        <span>{d.label}</span>
                        <span>{d.score.toFixed(0)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </div>

              {/* Roots Manager */}
              <section className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
                <h2 className="mb-3 text-sm font-semibold">Configured Scan Roots</h2>
                <form onSubmit={handleAddRoot} className="mb-4 flex gap-2">
                  <input
                    type="text"
                    value={newRootPath}
                    onChange={(e) => setNewRootPath(e.target.value)}
                    placeholder="e.g. C:\Users\you\Documents"
                    aria-label="New root folder path"
                    className="flex-1 rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
                  />
                  <button
                    type="button"
                    disabled={!desktop || busy}
                    onClick={() => void handleBrowseRoot()}
                    className="flex items-center gap-1.5 rounded-md border border-slate-700 px-3 py-1.5 text-sm font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                  >
                    <FolderOpen className="h-4 w-4" aria-hidden="true" />
                    Browse…
                  </button>
                  <button
                    type="submit"
                    disabled={!desktop || busy || !newRootPath.trim()}
                    className="rounded-md bg-slate-800 px-4 py-1.5 text-sm font-medium text-slate-100 hover:bg-slate-700 disabled:opacity-40"
                  >
                    Add Root
                  </button>
                </form>
                {roots.length === 0 ? (
                  <p className="text-xs text-slate-400">
                    No roots added yet. Add a folder path above to begin Tier-1
                    discovery.
                  </p>
                ) : (
                  <ul className="divide-y divide-slate-800">
                    {roots.map((r) => (
                      <li
                        key={r.id}
                        className="flex items-center justify-between py-2 text-sm"
                      >
                        <span className="font-mono text-xs text-slate-200">
                          {r.path}
                        </span>
                        <div className="flex items-center gap-2">
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() =>
                              void run("Toggle root", () =>
                                aegis.roots.toggle(r.path, !r.enabled)
                              )
                            }
                            className={`rounded px-2 py-0.5 text-xs font-medium disabled:opacity-40 ${
                              r.enabled
                                ? "bg-emerald-500/20 text-emerald-300"
                                : "bg-slate-800 text-slate-400"
                            }`}
                          >
                            {r.enabled ? "Enabled" : "Paused"}
                          </button>
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() =>
                              void run("Remove root", async () => {
                                await aegis.roots.remove(r.path);
                                return `Removed root ${r.path} (indexed data is kept).`;
                              })
                            }
                            className="rounded px-2 py-0.5 text-xs text-rose-400 hover:bg-rose-950/40 disabled:opacity-40"
                          >
                            Remove
                          </button>
                        </div>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            </div>
          )}

          {tab === "search" && (
            <div className="space-y-4">
              <form onSubmit={handleSearch} className="flex flex-wrap items-center gap-3">
                <input
                  ref={searchInputRef}
                  type="search"
                  value={query}
                  onChange={(e) => setQuery(e.target.value)}
                  placeholder="Search indexed files by keyword..."
                  aria-label="Search query"
                  className="min-w-64 flex-1 rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-sm text-slate-100 focus:border-emerald-500 focus:outline-none"
                />
                <label className="flex items-center gap-2 text-xs text-slate-300">
                  <input
                    type="checkbox"
                    checked={includeQuarantined}
                    onChange={(e) => setIncludeQuarantined(e.target.checked)}
                    className="rounded border-slate-700 bg-slate-900"
                  />
                  Include Quarantined (OFF by default)
                </label>
                <button
                  type="submit"
                  disabled={!desktop || busy}
                  className="rounded-lg bg-emerald-600 px-4 py-2 text-sm font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                >
                  Search
                </button>
              </form>

              {searchPage && (
                <div className="space-y-2">
                  <p className="text-xs text-slate-400">
                    {searchPage.total_hits} hit(s) in {searchPage.took_ms}ms
                  </p>
                  {searchPage.hits.map((hit) => (
                    <article
                      key={`${hit.file_id}-${hit.snippet}`}
                      className="rounded-lg border border-slate-800 bg-slate-900/60 p-3"
                    >
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <span className="font-semibold text-slate-100">
                          {hit.name}
                        </span>
                        <div className="flex items-center gap-1.5 text-xs">
                          <span className="rounded bg-slate-800 px-2 py-0.5 text-slate-300">
                            {hit.match_reason}
                          </span>
                          <span className="rounded bg-slate-800 px-2 py-0.5 text-slate-300">
                            state: {hit.state}
                          </span>
                        </div>
                      </div>
                      <p className="mt-1 font-mono text-xs text-slate-400">
                        {hit.path}
                      </p>
                      <p className="mt-2 text-xs text-slate-200">{hit.snippet}</p>
                    </article>
                  ))}
                </div>
              )}
            </div>
          )}

          {tab === "rescue" && (
            <div className="space-y-6">
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                <div>
                  <h2 className="text-sm font-semibold">
                    Disk Rescue & Per-Volume Quarantine
                  </h2>
                  <p className="text-xs text-slate-400">
                    Staged: <strong className="text-amber-300">{formatBytes(totalStagedBytes)}</strong>{" "}
                    · Actually Freed:{" "}
                    <strong className="text-emerald-400">{formatBytes(totalFreedBytes)}</strong>
                  </p>
                </div>
                <button
                  type="button"
                  disabled={!desktop || busy || quarantineEntries.length === 0}
                  onClick={() => setPurgeConfirmOpen(true)}
                  className="flex items-center gap-1.5 rounded-md bg-rose-700 px-3 py-1.5 text-xs font-semibold text-white hover:bg-rose-600 disabled:opacity-40"
                >
                  <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                  Empty Quarantine Now ({quarantineEntries.length})
                </button>
              </div>

              <section className="rounded-xl border border-slate-800 bg-slate-900/40 p-4">
                <h3 className="mb-3 flex items-center gap-2 text-sm font-semibold">
                  <FolderGit2 className="h-4 w-4 text-emerald-400" aria-hidden="true" />
                  Detected Build Caches (safe_build_cache carve-out)
                </h3>
                {rescue?.cache_dirs.length ? (
                  <div className="space-y-2">
                    {rescue.cache_dirs.map((c) => (
                      <div
                        key={c.id}
                        className="flex items-center justify-between rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs"
                      >
                        <div>
                          <div className="font-mono text-slate-200">{c.path}</div>
                          <div className="text-slate-400">
                            {c.cache_kind} · {c.file_count} files ·{" "}
                            {formatBytes(c.size_bytes)}
                          </div>
                        </div>
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => void handleStageCacheQuarantine(c)}
                          className="rounded bg-amber-600/80 px-3 py-1 font-medium text-white hover:bg-amber-500 disabled:opacity-40"
                        >
                          Propose Quarantine
                        </button>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="text-xs text-slate-400">No active build caches found.</p>
                )}
              </section>

              <section className="rounded-xl border border-slate-800 bg-slate-900/40 p-4">
                <h3 className="mb-3 text-sm font-semibold">
                  Exact Duplicates (Hardlink-Adjusted)
                </h3>
                {rescue?.exact_dupe_clusters.length ? (
                  <div className="space-y-2 text-xs">
                    {rescue.exact_dupe_clusters.map((cl) => (
                      <div
                        key={cl.cluster_key}
                        className="flex items-center justify-between rounded border border-slate-800 bg-slate-950/60 p-2.5"
                      >
                        <span>
                          Cluster {cl.cluster_key} ({cl.member_file_ids.length} files){" "}
                          {cl.hardlink_adjusted && (
                            <span className="ml-1 rounded bg-slate-800 px-1.5 py-0.5 text-amber-300">
                              Hardlink-adjusted
                            </span>
                          )}
                        </span>
                        <span className="flex items-center gap-3">
                          <span className="font-semibold text-emerald-400">
                            {formatBytes(cl.reclaimable_bytes)} reclaimable
                          </span>
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => void handleProposeDupes(cl)}
                            className="flex items-center gap-1 rounded bg-amber-600/80 px-2.5 py-1 font-medium text-white hover:bg-amber-500 disabled:opacity-40"
                          >
                            <Copy className="h-3 w-3" aria-hidden="true" />
                            Quarantine extra copies
                          </button>
                        </span>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="text-xs text-slate-400">No exact duplicate clusters.</p>
                )}
              </section>

              <section className="rounded-xl border border-slate-800 bg-slate-900/40 p-4">
                <h3 className="mb-3 text-sm font-semibold">Active Quarantine Items</h3>
                {quarantineEntries.length ? (
                  <div className="space-y-2 text-xs">
                    {quarantineEntries.map((q) => (
                      <div
                        key={q.id}
                        className="flex items-center justify-between rounded border border-slate-800 bg-slate-950/60 p-2.5"
                      >
                        <div>
                          <div className="font-mono text-slate-200">{q.original_path}</div>
                          <div className="text-slate-400">
                            {q.target_kind} · {formatBytes(q.size_bytes)}
                          </div>
                        </div>
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => void handleRestore(q)}
                          className="flex items-center gap-1 rounded bg-slate-800 px-2.5 py-1 text-slate-200 hover:bg-slate-700 disabled:opacity-40"
                        >
                          <RotateCcw className="h-3 w-3" /> Restore
                        </button>
                      </div>
                    ))}
                  </div>
                ) : (
                  <p className="text-xs text-slate-400">Quarantine is empty.</p>
                )}
              </section>
            </div>
          )}

          {tab === "simulator" && (
            <div className="space-y-5">
              {simulation && (
                <section className="rounded-xl border border-emerald-500/30 bg-emerald-950/15 p-4">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <h2 className="text-sm font-semibold">
                      Dry-Run Simulation: {simulation.batch_id.slice(0, 8)}
                    </h2>
                    <span className="text-xs text-slate-300">
                      Ready: {simulation.ready_count} · Review: {simulation.review_count} ·
                      Blocked: {simulation.blocked_count}
                    </span>
                  </div>
                  <div className="mt-3 space-y-2">
                    {simulation.items.map((item) => (
                      <div
                        key={item.action_id}
                        className="flex flex-wrap items-center justify-between gap-2 rounded border border-slate-800 bg-slate-950/70 p-2.5 text-xs"
                      >
                        <div>
                          <div className="font-mono text-slate-200">
                            {item.from_path} → {item.effective_to_path ?? "(quarantine)"}
                          </div>
                          <div className="text-slate-400">{item.reason}</div>
                        </div>
                        <div className="flex items-center gap-2">
                          {item.atomic_unit_protected && (
                            <span className="flex items-center gap-1 rounded bg-rose-950 px-2 py-0.5 text-rose-300">
                              <Lock className="h-3 w-3" /> Atomic Unit
                            </span>
                          )}
                          <span
                            className={`rounded px-2 py-0.5 font-semibold ${
                              item.confidence >= 0.9
                                ? "bg-emerald-500/20 text-emerald-300"
                                : item.confidence >= 0.6
                                  ? "bg-amber-500/20 text-amber-300"
                                  : "bg-slate-800 text-slate-400"
                            }`}
                          >
                            {(item.confidence * 100).toFixed(0)}% · {item.policy_verdict}
                          </span>
                        </div>
                      </div>
                    ))}
                  </div>
                </section>
              )}

              <section className="space-y-3">
                <h2 className="text-sm font-semibold">Action Batches</h2>
                {groupedBatches.length === 0 ? (
                  <p className="text-xs text-slate-400">No proposed or executed batches.</p>
                ) : (
                  groupedBatches.map(([batchId, items]) => {
                    const count = (s: ActionItem["status"]) =>
                      items.filter((a) => a.status === s).length;
                    const proposed = count("proposed");
                    const approved = count("approved");
                    const executed = count("executed");
                    const sel = selected[batchId] ?? [];
                    const btn =
                      "rounded px-2.5 py-1 disabled:cursor-not-allowed disabled:opacity-35";
                    return (
                      <div
                        key={batchId}
                        className="rounded-xl border border-slate-800 bg-slate-900/50 p-4"
                      >
                        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                          <span className="font-mono text-xs text-slate-300">
                            Batch {batchId.slice(0, 8)} ({items.length} action(s))
                          </span>
                          <div className="flex flex-wrap gap-2 text-xs">
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => void handleSimulate(batchId)}
                              className={`${btn} bg-slate-800 hover:bg-slate-700`}
                            >
                              Simulate
                            </button>
                            {sel.length > 0 ? (
                              <button
                                type="button"
                                disabled={busy}
                                onClick={() => void handleApprove(batchId, sel)}
                                className={`${btn} bg-emerald-700 text-white hover:bg-emerald-600`}
                              >
                                Approve selected ({sel.length})
                              </button>
                            ) : (
                              <button
                                type="button"
                                disabled={busy || proposed === 0}
                                title={proposed === 0 ? "Nothing awaiting approval" : undefined}
                                onClick={() => void handleApprove(batchId)}
                                className={`${btn} bg-emerald-700 text-white hover:bg-emerald-600`}
                              >
                                Approve all
                              </button>
                            )}
                            <button
                              type="button"
                              disabled={busy || proposed + approved === 0}
                              onClick={() => void handleReject(batchId)}
                              className={`${btn} border border-slate-700 hover:bg-slate-800`}
                            >
                              Reject
                            </button>
                            <button
                              type="button"
                              disabled={busy || approved === 0}
                              title={approved === 0 ? "Approve actions first" : undefined}
                              onClick={() => void handleExecute(batchId)}
                              className={`${btn} bg-emerald-600 font-semibold text-white hover:bg-emerald-500`}
                            >
                              Execute{approved > 0 ? ` (${approved})` : ""}
                            </button>
                            <button
                              type="button"
                              disabled={busy || executed === 0}
                              onClick={() => void handleRollbackBatch(batchId)}
                              className={`${btn} bg-amber-700 text-white hover:bg-amber-600`}
                            >
                              Rollback
                            </button>
                          </div>
                        </div>
                        <ul className="space-y-1.5 text-xs">
                          {items.map((a) => (
                            <li
                              key={a.id}
                              className="rounded bg-slate-950/60 px-3 py-2"
                            >
                              <div className="flex items-center justify-between gap-2">
                                <label className="flex min-w-0 items-center gap-2">
                                  {a.status === "proposed" && (
                                    <input
                                      type="checkbox"
                                      aria-label={`Select ${a.from_path}`}
                                      checked={sel.includes(a.id)}
                                      onChange={(e) =>
                                        setSelected((prev) => {
                                          const cur = prev[batchId] ?? [];
                                          return {
                                            ...prev,
                                            [batchId]: e.target.checked
                                              ? [...cur, a.id]
                                              : cur.filter((x) => x !== a.id),
                                          };
                                        })
                                      }
                                      className="rounded border-slate-700 bg-slate-900"
                                    />
                                  )}
                                  <span className="truncate font-mono text-slate-300">
                                    [{a.action_type}] {a.from_path}
                                  </span>
                                </label>
                                <span className="ml-2 shrink-0 rounded bg-slate-800 px-2 py-0.5 text-slate-200">
                                  {a.status}
                                </span>
                              </div>
                              {a.error_message && (
                                <div className="mt-1 text-rose-300">{a.error_message}</div>
                              )}
                            </li>
                          ))}
                        </ul>
                      </div>
                    );
                  })
                )}
              </section>
            </div>
          )}

          {tab === "doctor" && (
            <div className="space-y-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <h2 className="text-sm font-semibold">
                    Aegis Doctor (8-Point Integrity Matrix)
                  </h2>
                  <p className="text-xs text-slate-400">
                    Check #1 (Audit Chain) is strictly read-only and never auto-rewrites.
                  </p>
                </div>
                <div className="flex gap-2">
                  <button
                    type="button"
                    disabled={!desktop || busy}
                    onClick={() => void handleRunDoctor(false)}
                    className="rounded-md border border-slate-700 px-3 py-1.5 text-xs font-medium hover:bg-slate-800 disabled:opacity-40"
                  >
                    Run Diagnostics
                  </button>
                  <button
                    type="button"
                    disabled={!desktop || busy}
                    onClick={() => void handleRunDoctor(true)}
                    className="rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                  >
                    Auto-Repair Safe Drift
                  </button>
                </div>
              </div>

              {doctorReport && (
                <div className="space-y-2">
                  {doctorReport.checks.map((c) => (
                    <div
                      key={c.check_id}
                      className="flex items-start justify-between rounded-lg border border-slate-800 bg-slate-900/60 p-3 text-xs"
                    >
                      <div className="flex items-start gap-2.5">
                        {c.passed ? (
                          <CheckCircle2 className="mt-0.5 h-4 w-4 text-emerald-400" />
                        ) : (
                          <XCircle className="mt-0.5 h-4 w-4 text-rose-400" />
                        )}
                        <div>
                          <div className="font-semibold text-slate-100">
                            #{c.check_id} {c.name}
                          </div>
                          <div className="text-slate-400">{c.details}</div>
                          {c.remediation && (
                            <div className="mt-1 text-amber-300">{c.remediation}</div>
                          )}
                        </div>
                      </div>
                      <span
                        className={`rounded px-2 py-0.5 font-medium ${
                          c.passed
                            ? "bg-emerald-500/20 text-emerald-300"
                            : "bg-rose-500/20 text-rose-300"
                        }`}
                      >
                        {c.repaired ? "Repaired" : c.passed ? "Passed" : "Failed"}
                      </span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      </main>

      {/* Coverage Report Modal */}
      {coverageModal && (
        <div
          role="dialog"
          aria-modal="true"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
        >
          <div className="max-h-[80vh] w-full max-w-2xl overflow-y-auto rounded-xl border border-slate-700 bg-slate-900 p-5">
            <div className="mb-3 flex items-center justify-between">
              <h2 className="text-sm font-semibold">
                Honest Coverage Report — {coverageModal.coverage_pct.toFixed(1)}%
              </h2>
              <button
                type="button"
                onClick={() => setCoverageModal(null)}
                className="text-xs text-slate-400 hover:text-white"
              >
                Close (Esc)
              </button>
            </div>
            <p className="mb-3 text-xs text-slate-300">
              Root: {coverageModal.root_path} · Indexed {coverageModal.indexed} /{" "}
              {coverageModal.total_seen} · Skipped perm: {coverageModal.skipped_perm} ·
              Cloud placeholders: {coverageModal.skipped_cloud}
            </p>
            {coverageModal.errors.length === 0 ? (
              <p className="text-xs text-slate-400">No permission or read errors.</p>
            ) : (
              <ul className="space-y-1.5 text-xs">
                {coverageModal.errors.map((err, i) => (
                  <li key={i} className="rounded bg-slate-950 p-2 font-mono text-amber-300">
                    [{err.code}] {err.path} — {err.message}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </div>
      )}

      {/* Permanent Purge Confirmation Modal */}
      {purgeConfirmOpen && (
        <div
          role="dialog"
          aria-modal="true"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
        >
          <div className="w-full max-w-md rounded-xl border border-rose-500/40 bg-slate-900 p-5">
            <h2 className="flex items-center gap-2 text-sm font-semibold text-rose-300">
              <ShieldAlert className="h-4 w-4" /> Confirm Permanent Purge
            </h2>
            <p className="mt-2 text-xs text-slate-300">
              This will permanently delete {quarantineEntries.length} staged item(s) (
              {formatBytes(totalStagedBytes)}) from volume quarantine and free disk space.
              This cannot be undone.
            </p>
            <div className="mt-4 flex justify-end gap-2 text-xs">
              <button
                type="button"
                onClick={() => setPurgeConfirmOpen(false)}
                className="rounded border border-slate-700 px-3 py-1.5 hover:bg-slate-800"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={() => void handlePurgeAllQuarantine()}
                className="rounded bg-rose-600 px-3 py-1.5 font-semibold text-white hover:bg-rose-500"
              >
                Permanently Delete
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Pre-Rollback 3-Way Conflict Dialog (`D3 §4`, `C1 v1.2`) */}
      {conflicts.length > 0 && (
        <div
          role="dialog"
          aria-modal="true"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4"
        >
          <div className="max-h-[85vh] w-full max-w-2xl overflow-y-auto rounded-xl border border-amber-500/40 bg-slate-900 p-5">
            <div className="flex items-start justify-between gap-3">
              <div>
                <h2 className="text-sm font-semibold text-amber-300">
                  Cannot cleanly revert {conflicts.length} file(s) — external changes
                  detected
                </h2>
                <p className="mt-1 text-xs text-slate-400">
                  Unaffected items stay where they are until every conflict below is
                  resolved; then the rest of the batch is rolled back automatically.
                </p>
              </div>
              <button
                type="button"
                onClick={closeConflicts}
                className="shrink-0 text-xs text-slate-400 hover:text-white"
              >
                Close (Esc)
              </button>
            </div>
            <div className="mt-4 space-y-3">
              {conflicts.map((c) => {
                const options: { value: ConflictResolutionChoice; label: string }[] =
                  c.conflict === "modified"
                    ? [
                        {
                          value: "copy_back_keeping_both",
                          label: "Copy edited file back & keep both",
                        },
                        {
                          value: "revert_keeping_edits",
                          label: "Move edited file back to original path",
                        },
                        { value: "skip", label: "Skip this file" },
                        { value: "abort_batch", label: "Abort entire rollback" },
                      ]
                    : c.conflict === "collision"
                      ? [
                          {
                            value: "restore_as_rename",
                            label: "Restore as renamed copy (.restored-<ts>)",
                          },
                          { value: "skip", label: "Skip this file" },
                          { value: "abort_batch", label: "Abort entire rollback" },
                        ]
                      : [
                          {
                            value: "skip",
                            label: "Skip (mark unrecoverable_missing)",
                          },
                          { value: "abort_batch", label: "Abort entire rollback" },
                        ];
                return (
                  <div
                    key={c.action_id}
                    className="rounded-lg border border-slate-800 bg-slate-950/70 p-3 text-xs"
                  >
                    <div className="font-mono text-slate-200">{c.from_path}</div>
                    <div className="mt-0.5 text-amber-300">{c.detail}</div>
                    <div className="mt-2 flex flex-wrap items-center gap-2">
                      <select
                        value={conflictChoices[c.action_id] ?? c.recommended_choice}
                        onChange={(e) =>
                          setConflictChoices((prev) => ({
                            ...prev,
                            [c.action_id]: e.target
                              .value as ConflictResolutionChoice,
                          }))
                        }
                        className="rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                      >
                        {options.map((o) => (
                          <option key={o.value} value={o.value}>
                            {o.label}
                          </option>
                        ))}
                      </select>
                      <button
                        type="button"
                        disabled={busy}
                        onClick={() => handleResolveConflict(c.action_id)}
                        className="rounded bg-amber-600 px-3 py-1 font-semibold text-white hover:bg-amber-500 disabled:opacity-40"
                      >
                        Apply
                      </button>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
