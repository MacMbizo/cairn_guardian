import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Activity,
  AlertTriangle,
  AppWindow,
  CheckCircle2,
  ExternalLink,
  FolderX,
  HardDrive,
  HeartPulse,
  Power,
  RefreshCw,
  ShieldCheck,
  Sparkles,
  Trash2,
  TrendingUp,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type CleanupCandidate,
  type CleanupScanReport,
  type DiskTrendsReport,
  type DriveHealthItem,
  type HealthBreakdown,
  type InstalledApp,
  type LeftoverFolder,
  type StartupEntry,
} from "../bindings/client";
import { formatBytes } from "../format";
import {
  PREVIEW_APPS,
  PREVIEW_CLEANUP,
  PREVIEW_DRIVES,
  PREVIEW_LEFTOVERS,
  PREVIEW_STARTUP,
  PREVIEW_TRENDS,
} from "./fixtures/care";

type CareSection = "overview" | "cleanup" | "apps" | "startup" | "health" | "trends";

function VolumeSparkline({ points }: { points: { day: string; used_bytes: number; total_bytes: number }[] }) {
  if (points.length < 2) {
    return (
      <p className="mt-2 text-xs text-slate-500">
        Collecting daily samples (at least 2 days needed to draw a trend line).
      </p>
    );
  }
  const w = 360;
  const h = 68;
  const pad = 6;
  const values = points.map((p) => p.used_bytes);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = Math.max(max - min, 1);
  const coords = points.map((p, i) => {
    const x = pad + (i / (points.length - 1)) * (w - pad * 2);
    const y = h - pad - ((p.used_bytes - min) / span) * (h - pad * 2);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const area = `${pad},${h - pad} ${coords.join(" ")} ${w - pad},${h - pad}`;

  return (
    <div className="mt-3">
      <svg
        viewBox={`0 0 ${w} ${h}`}
        className="h-16 w-full overflow-visible rounded bg-slate-950/70 p-1"
        role="img"
        aria-label="Disk usage over time"
      >
        <polygon points={area} fill="rgba(16, 185, 129, 0.16)" />
        <polyline
          fill="none"
          stroke="rgb(52, 211, 153)"
          strokeWidth="2"
          strokeLinejoin="round"
          strokeLinecap="round"
          points={coords.join(" ")}
        />
      </svg>
      <div className="mt-1 flex justify-between text-xs text-slate-500">
        <span>{points[0].day}</span>
        <span>{points[points.length - 1].day}</span>
      </div>
    </div>
  );
}

export function SystemCare({
  desktop,
  health,
  onReviewBatches,
}: {
  desktop: boolean;
  health: HealthBreakdown | null;
  onReviewBatches: () => void;
}) {
  const [section, setSection] = useState<CareSection>("overview");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const [cleanup, setCleanup] = useState<CleanupScanReport | null>(desktop ? null : PREVIEW_CLEANUP);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [apps, setApps] = useState<InstalledApp[]>(desktop ? [] : PREVIEW_APPS);
  const [leftovers, setLeftovers] = useState<LeftoverFolder[]>(desktop ? [] : PREVIEW_LEFTOVERS);
  const [appQuery, setAppQuery] = useState("");
  const [appFilter, setAppFilter] = useState<"all" | "unused" | "large">("all");
  const [startup, setStartup] = useState<StartupEntry[]>(desktop ? [] : PREVIEW_STARTUP);
  const [drives, setDrives] = useState<DriveHealthItem[]>(desktop ? [] : PREVIEW_DRIVES);
  const [trends, setTrends] = useState<DiskTrendsReport | null>(desktop ? null : PREVIEW_TRENDS);
  const [trendDays, setTrendDays] = useState<30 | 90>(30);

  const loadCleanup = useCallback(async () => {
    if (!desktop) {
      setCleanup(PREVIEW_CLEANUP);
      return;
    }
    const rep = await aegis.care.scanCleanup();
    setCleanup(rep);
    setSelectedIds((prev) => {
      if (prev.size > 0) return prev;
      // Pre-select safe temp/crash/browser/dev caches; leave DeadFiles opt-in by explicit review.
      const next = new Set<string>();
      for (const g of rep.groups) {
        if (g.category !== "dead_files") {
          for (const it of g.items) next.add(it.id);
        }
      }
      return next;
    });
  }, [desktop]);

  const loadAppsAndLeftovers = useCallback(async () => {
    if (!desktop) {
      setApps(PREVIEW_APPS);
      setLeftovers(PREVIEW_LEFTOVERS);
      return;
    }
    const [appList, leftoverList] = await Promise.all([
      aegis.care.installedApps(),
      aegis.care.leftoverFolders(),
    ]);
    setApps(appList);
    setLeftovers(leftoverList);
  }, [desktop]);

  const loadStartup = useCallback(async () => {
    if (!desktop) {
      setStartup(PREVIEW_STARTUP);
      return;
    }
    setStartup(await aegis.care.startupItems());
  }, [desktop]);

  const loadDrives = useCallback(async () => {
    if (!desktop) {
      setDrives(PREVIEW_DRIVES);
      return;
    }
    setDrives(await aegis.care.driveHealth());
  }, [desktop]);

  const loadTrends = useCallback(
    async (days: number) => {
      if (!desktop) {
        setTrends(PREVIEW_TRENDS);
        return;
      }
      setTrends(await aegis.care.diskTrends(days));
    },
    [desktop]
  );

  const runTask = useCallback(async (fn: () => Promise<void>) => {
    setBusy(true);
    setNotice(null);
    try {
      await fn();
    } catch (e) {
      setNotice(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }, []);

  useEffect(() => {
    if (!desktop) return;
    void runTask(async () => {
      await Promise.all([
        loadCleanup(),
        loadDrives(),
        loadTrends(trendDays),
        loadAppsAndLeftovers(),
        loadStartup(),
      ]);
    });
  }, [desktop, loadAppsAndLeftovers, loadCleanup, loadDrives, loadStartup, loadTrends, runTask, trendDays]);

  const allCandidates = useMemo(
    () => cleanup?.groups.flatMap((g) => g.items) ?? [],
    [cleanup]
  );
  const selectedItems = useMemo(
    () => allCandidates.filter((c) => selectedIds.has(c.id)),
    [allCandidates, selectedIds]
  );
  const selectedBytes = useMemo(
    () => selectedItems.reduce((sum, i) => sum + i.bytes, 0),
    [selectedItems]
  );

  const toggleItem = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const toggleGroup = (items: CleanupCandidate[]) => {
    const allChecked = items.every((i) => selectedIds.has(i.id));
    setSelectedIds((prev) => {
      const next = new Set(prev);
      for (const i of items) {
        if (allChecked) next.delete(i.id);
        else next.add(i.id);
      }
      return next;
    });
  };

  const handleProposeSelected = () =>
    void runTask(async () => {
      if (selectedItems.length === 0) return;
      const batchId = await aegis.care.proposeCleanup(selectedItems);
      setNotice(
        `Queued ${selectedItems.length} item(s) (${formatBytes(selectedBytes)}) in batch ${batchId.slice(0, 8)} for 30-day quarantine review.`
      );
      await loadCleanup();
      onReviewBatches();
    });

  const handleProposeLeftover = (f: LeftoverFolder) =>
    void runTask(async () => {
      const item: CleanupCandidate = {
        id: `leftover:${f.path}`,
        category: "uninstall_leftover",
        group_label: f.parent_label,
        path: f.path,
        target_kind: "dir",
        bytes: f.bytes,
        file_count: f.file_count,
        age_days: f.age_days,
        reason: f.reason,
        file_id: null,
        safety_note: "Moves only the leftover profile folder to the 30-day quarantine.",
      };
      const batchId = await aegis.care.proposeCleanup([item]);
      setNotice(
        `Queued leftover folder "${f.folder_name}" (${formatBytes(f.bytes)}) in batch ${batchId.slice(0, 8)}.`
      );
      onReviewBatches();
    });

  const filteredApps = useMemo(() => {
    const q = appQuery.trim().toLowerCase();
    return apps.filter((a) => {
      if (appFilter === "unused" && !a.unused_likely) return false;
      if (appFilter === "large" && a.estimated_bytes < 100 * 1024 * 1024) return false;
      if (!q) return true;
      return (
        a.name.toLowerCase().includes(q) ||
        (a.publisher ?? "").toLowerCase().includes(q)
      );
    });
  }, [apps, appFilter, appQuery]);

  const unusedCount = useMemo(() => apps.filter((a) => a.unused_likely).length, [apps]);
  const orphanedStartupCount = useMemo(
    () => startup.filter((s) => s.orphaned).length,
    [startup]
  );

  const subTabs: { id: CareSection; label: string; icon: typeof HeartPulse }[] = [
    { id: "overview", label: "Overview & Health", icon: HeartPulse },
    { id: "cleanup", label: "Cleanup", icon: Sparkles },
    { id: "apps", label: "Apps & Leftovers", icon: AppWindow },
    { id: "startup", label: "Startup Items", icon: Power },
    { id: "health", label: "Drive Health (SMART)", icon: HardDrive },
    { id: "trends", label: "Disk Trends", icon: TrendingUp },
  ];

  return (
    <div className="space-y-5">
      <header className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <div>
          <h1 className="flex items-center gap-2.5 text-lg font-semibold text-slate-50">
            <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-slate-800 bg-slate-900 text-emerald-400">
              <HeartPulse className="h-4 w-4" aria-hidden="true" />
            </span>
            Windows System Care
          </h1>
          <p className="mt-1.5 max-w-3xl text-sm leading-relaxed text-slate-400">
            Reclaim clutter via 30-day quarantine, uninstall apps with their own uninstallers,
            check SMART drive health, and track disk growth. Cairn never edits the registry.
          </p>
        </div>
        <button
          type="button"
          disabled={!desktop || busy}
          onClick={() =>
            void runTask(async () => {
              await Promise.all([
                loadCleanup(),
                loadDrives(),
                loadTrends(trendDays),
                loadAppsAndLeftovers(),
                loadStartup(),
              ]);
            })
          }
          className="flex items-center gap-1.5 rounded-md border border-slate-700 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${busy ? "animate-spin text-emerald-400" : ""}`} />
          Refresh All
        </button>
      </header>

      <div className="flex flex-wrap gap-2 border-b border-slate-800 pb-3">
        {subTabs.map((t) => {
          const Icon = t.icon;
          const active = section === t.id;
          return (
            <button
              key={t.id}
              type="button"
              onClick={() => setSection(t.id)}
              className={`flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
                active
                  ? "bg-emerald-500/20 text-emerald-300 ring-1 ring-emerald-500/40"
                  : "border border-slate-800 bg-slate-900/50 text-slate-300 hover:bg-slate-800"
              }`}
            >
              <Icon className="h-3.5 w-3.5" aria-hidden="true" />
              {t.label}
            </button>
          );
        })}
      </div>

      {notice && (
        <div
          role="status"
          className="rounded-lg border border-emerald-700/60 bg-emerald-950/30 px-3 py-2 text-xs text-emerald-200"
        >
          {notice}
        </div>
      )}

      {section === "overview" && (
        <div className="space-y-5">
          <div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-4">
            <button
              type="button"
              onClick={() => setSection("cleanup")}
              className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 text-left hover:border-emerald-700/60"
            >
              <div className="text-xs text-slate-400">Reclaimable Clutter</div>
              <div className="mt-1 text-xl font-semibold text-emerald-300">
                {formatBytes(cleanup?.total_reclaimable_bytes ?? 0)}
              </div>
              <div className="mt-1 text-xs text-slate-400">
                {allCandidates.length} items across {cleanup?.groups.length ?? 0} categories
              </div>
            </button>

            <button
              type="button"
              onClick={() => setSection("apps")}
              className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 text-left hover:border-emerald-700/60"
            >
              <div className="text-xs text-slate-400">Apps & Leftovers</div>
              <div className="mt-1 text-xl font-semibold text-slate-100">
                {apps.length} apps
              </div>
              <div className="mt-1 text-xs text-slate-400">
                {unusedCount} likely unused · {leftovers.length} leftover folder(s)
              </div>
            </button>

            <button
              type="button"
              onClick={() => setSection("startup")}
              className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 text-left hover:border-emerald-700/60"
            >
              <div className="text-xs text-slate-400">Startup Entries</div>
              <div className="mt-1 text-xl font-semibold text-slate-100">
                {startup.filter((s) => s.enabled).length} enabled
              </div>
              <div className="mt-1 text-xs text-slate-400">
                {orphanedStartupCount > 0
                  ? `${orphanedStartupCount} orphaned entry(s) spotted`
                  : `${startup.length} total read-only entries`}
              </div>
            </button>

            <button
              type="button"
              onClick={() => setSection("health")}
              className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 text-left hover:border-emerald-700/60"
            >
              <div className="text-xs text-slate-400">Physical Drives</div>
              <div className="mt-1 text-xl font-semibold text-slate-100">
                {drives.length} checked
              </div>
              <div className="mt-1 text-xs text-slate-400">
                {drives.some((d) => d.health_status.toLowerCase() !== "healthy")
                  ? "Warning reported — inspect Drive Health"
                  : "All reported healthy"}
              </div>
            </button>
          </div>

          {health && (
            <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <h3 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
                    <Activity className="h-4 w-4 text-emerald-400" aria-hidden="true" />
                    Workspace Health Score
                  </h3>
                  <p className="text-xs text-slate-400">
                    Storage headroom, drive/backup integrity, duplicates, caches/dead files, and review backlog.
                  </p>
                </div>
                <span className="rounded-lg border border-emerald-700/60 bg-emerald-950/50 px-3 py-1 text-lg font-bold text-emerald-300">
                  {health.overall_score.toFixed(1)} / 100
                </span>
              </div>
              <div className="mt-4 grid grid-cols-1 gap-3 md:grid-cols-5">
                {health.dimensions.map((d) => (
                  <div
                    key={d.id}
                    className="rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs"
                  >
                    <div className="font-medium text-slate-200">{d.label}</div>
                    <div className="mt-1 text-base font-semibold text-emerald-300">
                      {d.score.toFixed(0)}
                    </div>
                    <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-slate-800">
                      <div
                        className={`h-full rounded-full ${
                          d.score >= 80
                            ? "bg-emerald-500"
                            : d.score >= 55
                              ? "bg-amber-400"
                              : "bg-rose-500"
                        }`}
                        style={{ width: `${Math.max(4, Math.min(100, d.score))}%` }}
                      />
                    </div>
                    <div className="mt-1 text-xs text-slate-400">
                      {d.offenders_count} item(s)
                    </div>
                  </div>
                ))}
              </div>
            </section>
          )}
        </div>
      )}

      {section === "cleanup" && (
        <div className="space-y-4">
          <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <div>
              <h3 className="text-sm font-semibold text-slate-100">
                Selected to Quarantine: {formatBytes(selectedBytes)} ({selectedItems.length} item(s))
              </h3>
              <p className="text-xs text-slate-400">
                Creates one reviewable batch in Simulator & Actions (30-day quarantine, one-click undo). Nothing is deleted directly.
              </p>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                disabled={!desktop || busy}
                onClick={() => void runTask(loadCleanup)}
                className="rounded-md border border-slate-700 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
              >
                Re-scan
              </button>
              <button
                type="button"
                disabled={!desktop || busy || selectedItems.length === 0}
                onClick={handleProposeSelected}
                className="flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
              >
                <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                Propose Quarantine ({formatBytes(selectedBytes)})
              </button>
            </div>
          </div>

          {!cleanup || cleanup.groups.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-6 text-center text-xs text-slate-400">
              No aged temp files, caches, or untracked dead large files found right now.
            </div>
          ) : (
            cleanup.groups.map((grp) => {
              const allChecked = grp.items.every((i) => selectedIds.has(i.id));
              return (
                <section
                  key={grp.category}
                  className="rounded-xl border border-slate-800 bg-slate-900/60 p-4"
                >
                  <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <h4 className="text-sm font-semibold text-slate-100">
                        {grp.label} ·{" "}
                        <span className="text-emerald-300">{formatBytes(grp.total_bytes)}</span>{" "}
                        <span className="text-xs font-normal text-slate-400">
                          ({grp.item_count} item{grp.item_count === 1 ? "" : "s"})
                        </span>
                      </h4>
                      <p className="text-xs text-slate-400">{grp.description}</p>
                    </div>
                    <button
                      type="button"
                      onClick={() => toggleGroup(grp.items)}
                      className="rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
                    >
                      {allChecked ? "Clear group" : "Select group"}
                    </button>
                  </div>
                  <div className="max-h-72 space-y-1.5 overflow-y-auto pr-1 text-xs">
                    {grp.items.map((it) => (
                      <label
                        key={it.id}
                        className="flex cursor-pointer items-start justify-between gap-3 rounded-lg border border-slate-800 bg-slate-950/60 p-2.5 hover:border-slate-700"
                      >
                        <span className="flex items-start gap-2.5">
                          <input
                            type="checkbox"
                            checked={selectedIds.has(it.id)}
                            onChange={() => toggleItem(it.id)}
                            className="mt-0.5 accent-emerald-500"
                          />
                          <span>
                            <span className="block font-mono text-slate-200">{it.path}</span>
                            <span className="block text-xs text-slate-400">
                              {it.reason} · {it.file_count} file(s)
                            </span>
                          </span>
                        </span>
                        <span className="shrink-0 font-semibold text-emerald-300">
                          {formatBytes(it.bytes)}
                        </span>
                      </label>
                    ))}
                  </div>
                </section>
              );
            })
          )}
        </div>
      )}

      {section === "apps" && (
        <div className="space-y-5">
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-sky-800/50 bg-sky-950/20 px-4 py-2.5 text-xs text-sky-200">
            <span className="flex items-center gap-2">
              <ShieldCheck className="h-4 w-4 shrink-0 text-sky-400" aria-hidden="true" />
              Cairn never cleans or edits the Windows Registry. Uninstall launches each app&apos;s own Windows uninstaller; leftover folders go to 30-day quarantine.
            </span>
            <button
              type="button"
              disabled={!desktop || busy}
              onClick={() => void runTask(() => aegis.care.openWindowsApps())}
              className="flex items-center gap-1 rounded border border-sky-700/60 bg-sky-900/40 px-2.5 py-1 font-medium text-sky-100 hover:bg-sky-800/50 disabled:opacity-40"
            >
              Open Windows Apps <ExternalLink className="h-3 w-3" />
            </button>
          </div>

          <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
              <h3 className="text-sm font-semibold text-slate-100">
                Installed Applications ({filteredApps.length})
              </h3>
              <div className="flex flex-wrap items-center gap-2">
                <input
                  type="search"
                  value={appQuery}
                  onChange={(e) => setAppQuery(e.target.value)}
                  placeholder="Filter by app or publisher…"
                  className="rounded-md border border-slate-700 bg-slate-950 px-2.5 py-1 text-xs text-slate-200 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
                />
                {(["all", "unused", "large"] as const).map((f) => (
                  <button
                    key={f}
                    type="button"
                    onClick={() => setAppFilter(f)}
                    className={`rounded px-2.5 py-1 text-xs ${
                      appFilter === f
                        ? "bg-emerald-500/20 text-emerald-300"
                        : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                    }`}
                  >
                    {f === "all" ? "All" : f === "unused" ? "Likely Unused" : "Large (100 MB+)"}
                  </button>
                ))}
              </div>
            </div>

            <div className="max-h-96 space-y-1.5 overflow-y-auto pr-1 text-xs">
              {filteredApps.map((app) => (
                <div
                  key={app.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-2.5"
                >
                  <div>
                    <div className="flex flex-wrap items-center gap-2 font-medium text-slate-100">
                      <span>{app.name}</span>
                      {app.version && (
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 text-xs text-slate-400">
                          {app.version}
                        </span>
                      )}
                      {app.unused_likely && (
                        <span className="rounded border border-amber-700/60 bg-amber-950/50 px-1.5 py-0.5 text-xs text-amber-200">
                          Unused ~{app.days_since_used}d
                        </span>
                      )}
                    </div>
                    <div className="text-xs text-slate-400">
                      {app.publisher ?? "Unknown publisher"} · {app.scope}
                      {app.estimated_bytes > 0 ? ` · ${formatBytes(app.estimated_bytes)}` : ""}
                    </div>
                  </div>
                  <button
                    type="button"
                    disabled={!desktop || busy || !app.has_uninstaller}
                    onClick={() => void runTask(() => aegis.care.launchUninstaller(app.id))}
                    className="rounded border border-rose-700/60 bg-rose-950/40 px-2.5 py-1 font-medium text-rose-200 hover:bg-rose-900/50 disabled:opacity-40"
                  >
                    Uninstall
                  </button>
                </div>
              ))}
            </div>
          </section>

          <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold text-slate-100">
              <FolderX className="h-4 w-4 text-amber-300" aria-hidden="true" />
              Stale App Leftover Folders ({leftovers.length})
            </h3>
            {leftovers.length === 0 ? (
              <p className="text-xs text-slate-400">
                No aged unowned leftover folders detected inside AppData.
              </p>
            ) : (
              <div className="space-y-2 text-xs">
                {leftovers.map((lf) => (
                  <div
                    key={lf.path}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-2.5"
                  >
                    <div>
                      <div className="font-mono text-slate-200">{lf.path}</div>
                      <div className="text-xs text-slate-400">
                        {lf.reason} · {lf.file_count} file(s) · {formatBytes(lf.bytes)}
                      </div>
                    </div>
                    <button
                      type="button"
                      disabled={!desktop || busy}
                      onClick={() => handleProposeLeftover(lf)}
                      className="rounded bg-amber-600/80 px-2.5 py-1 font-medium text-white hover:bg-amber-500 disabled:opacity-40"
                    >
                      Propose Quarantine
                    </button>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      )}

      {section === "startup" && (
        <section className="space-y-4 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <div>
              <h3 className="text-sm font-semibold text-slate-100">
                Windows Startup Items ({startup.length}, read-only)
              </h3>
              <p className="text-xs text-slate-400">
                Cairn lists startup entries and flags ones whose program is missing. It never changes the registry. Switch items off in Windows Settings.
              </p>
            </div>
            <button
              type="button"
              disabled={!desktop || busy}
              onClick={() => void runTask(() => aegis.care.openStartupSettings())}
              className="flex items-center gap-1 rounded border border-emerald-700/60 bg-emerald-950/50 px-3 py-1.5 text-xs font-medium text-emerald-200 hover:bg-emerald-900/50 disabled:opacity-40"
            >
              Open Windows Startup Apps <ExternalLink className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="space-y-2 text-xs">
            {startup.map((s) => (
              <div
                key={s.id}
                className="flex flex-wrap items-start justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3"
              >
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2 font-medium text-slate-100">
                    <span>{s.name}</span>
                    <span
                      className={`rounded px-1.5 py-0.5 text-xs ${
                        s.enabled
                          ? "bg-emerald-500/20 text-emerald-300"
                          : "bg-slate-800 text-slate-400"
                      }`}
                    >
                      {s.enabled ? "Enabled" : "Disabled"}
                    </span>
                    {s.orphaned && (
                      <span className="rounded border border-amber-700/60 bg-amber-950/60 px-1.5 py-0.5 text-xs text-amber-200">
                        Orphaned target missing
                      </span>
                    )}
                    <span className="text-xs font-normal text-slate-400">{s.source}</span>
                  </div>
                  <p className="mt-1 truncate font-mono text-xs text-slate-400">
                    {s.command}
                  </p>
                  <p className="mt-0.5 text-xs text-slate-500">{s.impact_hint}</p>
                </div>
              </div>
            ))}
          </div>
        </section>
      )}

      {section === "health" && (
        <div className="space-y-4">
          {drives.map((d) => {
            const healthy = d.health_status.toLowerCase() === "healthy";
            return (
              <section
                key={d.device_id}
                className="rounded-xl border border-slate-800 bg-slate-900/60 p-4"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div>
                    <h3 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
                      <HardDrive className="h-4 w-4 text-emerald-400" aria-hidden="true" />
                      {d.model}
                    </h3>
                    <p className="text-xs text-slate-400">
                      {d.media_type} · {d.bus_type}
                      {d.size_bytes > 0 ? ` · ${formatBytes(d.size_bytes)}` : ""}
                      {d.drive_letters.length > 0 ? ` · Drives: ${d.drive_letters.join(", ")}` : ""}
                    </p>
                  </div>
                  <div className="flex items-center gap-2 text-xs">
                    <span
                      className={`flex items-center gap-1 rounded px-2 py-0.5 font-semibold ${
                        healthy
                          ? "bg-emerald-500/20 text-emerald-300"
                          : "bg-rose-500/20 text-rose-300"
                      }`}
                    >
                      {healthy ? (
                        <CheckCircle2 className="h-3.5 w-3.5" />
                      ) : (
                        <AlertTriangle className="h-3.5 w-3.5" />
                      )}
                      {d.health_status} ({d.operational_status})
                    </span>
                    <span className="rounded bg-slate-800 px-2 py-0.5 text-slate-300">
                      Catalog read errors: {d.catalog_read_errors}
                    </span>
                  </div>
                </div>

                {d.smart_detail_available ? (
                  <div className="mt-3 grid grid-cols-2 gap-2 text-xs sm:grid-cols-5">
                    <div className="rounded bg-slate-950/60 p-2">
                      <div className="text-slate-400">Temperature</div>
                      <div className="font-semibold text-slate-100">
                        {d.temperature_c != null ? `${d.temperature_c}°C` : "n/a"}
                      </div>
                    </div>
                    <div className="rounded bg-slate-950/60 p-2">
                      <div className="text-slate-400">SSD Wear</div>
                      <div className="font-semibold text-slate-100">
                        {d.wear_pct != null ? `${d.wear_pct}%` : "n/a"}
                      </div>
                    </div>
                    <div className="rounded bg-slate-950/60 p-2">
                      <div className="text-slate-400">Power-On Hours</div>
                      <div className="font-semibold text-slate-100">
                        {d.power_on_hours != null ? d.power_on_hours.toLocaleString() : "n/a"}
                      </div>
                    </div>
                    <div className="rounded bg-slate-950/60 p-2">
                      <div className="text-slate-400">Read Errors</div>
                      <div className="font-semibold text-slate-100">
                        {d.read_errors ?? 0}
                      </div>
                    </div>
                    <div className="rounded bg-slate-950/60 p-2">
                      <div className="text-slate-400">Write Errors</div>
                      <div className="font-semibold text-slate-100">
                        {d.write_errors ?? 0}
                      </div>
                    </div>
                  </div>
                ) : (
                  d.smart_note && (
                    <p className="mt-3 rounded border border-slate-800 bg-slate-950/50 px-3 py-2 text-xs text-slate-400">
                      {d.smart_note}
                    </p>
                  )
                )}
              </section>
            );
          })}
        </div>
      )}

      {section === "trends" && (
        <div className="space-y-5">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="text-sm font-semibold text-slate-100">
              Volume Space Trends ({trends?.today ?? "today"})
            </h3>
            <div className="flex gap-1.5 text-xs">
              {([30, 90] as const).map((w) => (
                <button
                  key={w}
                  type="button"
                  onClick={() => {
                    setTrendDays(w);
                    if (desktop) void runTask(() => loadTrends(w));
                  }}
                  className={`rounded px-2.5 py-1 ${
                    trendDays === w
                      ? "bg-emerald-500/20 text-emerald-300"
                      : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                  }`}
                >
                  {w} days
                </button>
              ))}
            </div>
          </div>

          <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
            {trends?.volumes.map((v) => {
              const usedPct =
                v.total_bytes > 0 ? Math.min(100, (v.used_bytes / v.total_bytes) * 100) : 0;
              return (
                <section
                  key={v.volume_id}
                  className="rounded-xl border border-slate-800 bg-slate-900/60 p-4"
                >
                  <div className="flex items-center justify-between gap-2">
                    <div>
                      <h4 className="text-sm font-semibold text-slate-100">
                        {v.label} ({v.mount_point})
                      </h4>
                      <p className="text-xs text-slate-400">
                        {formatBytes(v.used_bytes)} used of {formatBytes(v.total_bytes)} ·{" "}
                        {formatBytes(v.free_bytes)} free ({v.free_pct.toFixed(1)}%)
                      </p>
                    </div>
                    <span className="text-xs font-medium text-emerald-300">
                      {v.bytes_per_week >= 0 ? "+" : ""}
                      {formatBytes(Math.abs(v.bytes_per_week))}/wk
                    </span>
                  </div>
                  <div className="mt-2 h-2 overflow-hidden rounded-full bg-slate-800">
                    <div
                      className={`h-full rounded-full ${
                        usedPct >= 88
                          ? "bg-rose-500"
                          : usedPct >= 75
                            ? "bg-amber-400"
                            : "bg-emerald-500"
                      }`}
                      style={{ width: `${usedPct}%` }}
                    />
                  </div>
                  {v.weeks_left != null && (
                    <p className="mt-2 text-xs text-amber-200">
                      At this rate, {v.label} fills in about {Math.max(1, Math.round(v.weeks_left))}{" "}
                      week(s){v.fastest_root ? ` (fastest growing: ${v.fastest_root})` : ""}.
                    </p>
                  )}
                  <VolumeSparkline points={v.points} />
                </section>
              );
            })}
          </div>

          <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <h3 className="mb-3 text-sm font-semibold text-slate-100">
              Folder Growth ({trends?.roots.length ?? 0} tracked roots)
            </h3>
            {!trends?.roots.length ? (
              <p className="text-xs text-slate-400">
                Root space samples appear after daily scans complete.
              </p>
            ) : (
              <div className="space-y-2 text-xs">
                {trends.roots.map((r) => (
                  <div
                    key={r.root}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-2.5"
                  >
                    <div>
                      <div className="font-medium text-slate-100">{r.name}</div>
                      <div className="font-mono text-xs text-slate-400">{r.root}</div>
                    </div>
                    <div className="flex flex-wrap items-center gap-3 text-slate-300">
                      <span>Now: {formatBytes(r.current_bytes)}</span>
                      <span>7d: {r.delta_7d >= 0 ? "+" : "-"}{formatBytes(Math.abs(r.delta_7d))}</span>
                      <span>30d: {r.delta_30d >= 0 ? "+" : "-"}{formatBytes(Math.abs(r.delta_30d))}</span>
                      <span className="font-semibold text-emerald-300">
                        {r.bytes_per_week >= 0 ? "+" : "-"}{formatBytes(Math.abs(r.bytes_per_week))}/wk
                      </span>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      )}
    </div>
  );
}
