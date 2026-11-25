import { useCallback, useEffect, useMemo, useRef, useState, type FormEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ArchiveRestore,
  Check,
  Compass,
  Copy,
  EyeOff,
  FileSearch,
  FolderInput,
  HardDrive,
  RotateCcw,
  Search,
  Sparkles,
  Usb,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type AssetFamily,
  type FileDetail,
  type ForgottenAsset,
  type ForgottenAssetsReport,
} from "../bindings/client";
import { formatBytes } from "../format";
import { PREVIEW_FORGOTTEN_ASSETS } from "./fixtures/assets";

const DORMANCY_CHOICES: { label: string; days: number }[] = [
  { label: "Any age (0d+)", days: 0 },
  { label: "30d+ dormant", days: 30 },
  { label: "90d+ dormant", days: 90 },
  { label: "180d+ dormant", days: 180 },
  { label: "365d+ dormant", days: 365 },
];

const HEADER_ENRICHABLE_EXTS = new Set([
  "png",
  "jpg",
  "jpeg",
  "gif",
  "bmp",
  "webp",
  "psd",
  "psb",
  "svg",
]);

export function ForgottenAssets({
  desktop,
  onReviewBatches,
}: {
  desktop: boolean;
  onReviewBatches: () => void;
}) {
  const [report, setReport] = useState<ForgottenAssetsReport | null>(
    desktop ? null : PREVIEW_FORGOTTEN_ASSETS
  );
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const [queryInput, setQueryInput] = useState("");
  const [appliedQuery, setAppliedQuery] = useState("");
  const [familyFilter, setFamilyFilter] = useState<AssetFamily | null>(null);
  const [driveStatus, setDriveStatus] = useState<"all" | "online" | "offline">("all");
  const [volumeId, setVolumeId] = useState<string>("");
  const [minDormancyDays, setMinDormancyDays] = useState<number>(90);
  const [includeDismissed, setIncludeDismissed] = useState(false);

  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const enrichedOnceRef = useRef<Set<string>>(new Set());

  const loadScan = useCallback(async () => {
    if (!desktop) {
      setReport(PREVIEW_FORGOTTEN_ASSETS);
      return;
    }
    setLoading(true);
    setNotice(null);
    try {
      const next = await aegis.assets.scan({
        min_dormancy_days: minDormancyDays,
        families: familyFilter ? [familyFilter] : null,
        drive_status: driveStatus,
        volume_id: volumeId || null,
        query: appliedQuery || null,
        include_dismissed: includeDismissed,
        limit: 150,
      });
      setReport(next);
      setSelectedIds((prev) => {
        const valid = new Set(next.items.filter((i) => i.drive_online && !i.dismissed).map((i) => i.file_id));
        const kept = new Set<string>();
        for (const id of prev) {
          if (valid.has(id)) kept.add(id);
        }
        return kept;
      });
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [desktop, minDormancyDays, familyFilter, driveStatus, volumeId, appliedQuery, includeDismissed]);

  useEffect(() => {
    void loadScan();
  }, [loadScan]);

  // Lazily enrich visual dimensions & SVG palette headers for visible online images/design files once.
  useEffect(() => {
    if (!desktop || !report) return;
    const candidates = report.items
      .filter(
        (it) =>
          it.drive_online &&
          it.width === null &&
          HEADER_ENRICHABLE_EXTS.has(it.ext.toLowerCase()) &&
          !enrichedOnceRef.current.has(it.file_id)
      )
      .slice(0, 24)
      .map((it) => it.file_id);
    if (candidates.length === 0) return;

    for (const id of candidates) enrichedOnceRef.current.add(id);
    let cancelled = false;
    void aegis.assets
      .enrichMetadata(candidates)
      .then((count) => {
        if (!cancelled && count > 0) {
          void loadScan();
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [desktop, report, loadScan]);

  const displayedItems = useMemo(() => {
    if (!report) return [];
    if (desktop) return report.items;
    return report.items.filter((item) => {
      if (familyFilter && item.family !== familyFilter) return false;
      if (driveStatus === "online" && !item.drive_online) return false;
      if (driveStatus === "offline" && item.drive_online) return false;
      if (volumeId && item.volume_id !== volumeId) return false;
      if (!includeDismissed && item.dismissed) return false;
      if (appliedQuery) {
        const q = appliedQuery.toLowerCase();
        const hay = `${item.filename} ${item.path} ${item.matched_collection_name ?? ""} ${item.camera ?? ""}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [desktop, report, familyFilter, driveStatus, volumeId, includeDismissed, appliedQuery]);

  const selectedOnlineItems = useMemo(
    () => displayedItems.filter((it) => selectedIds.has(it.file_id) && it.drive_online && !it.dismissed),
    [displayedItems, selectedIds]
  );

  const selectedMatchedItems = useMemo(
    () => selectedOnlineItems.filter((it) => Boolean(it.suggested_destination)),
    [selectedOnlineItems]
  );

  const handleQuerySubmit = (e: FormEvent) => {
    e.preventDefault();
    setAppliedQuery(queryInput.trim());
  };

  const toggleSelect = (fileId: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(fileId)) next.delete(fileId);
      else next.add(fileId);
      return next;
    });
  };

  const proposeMoves = async (
    moves: { file_id: string; dest_dir: string; reason: string | null }[]
  ) => {
    if (!desktop) {
      setNotice("Preview mode: run the desktop app to stage recovery batches in the Simulator.");
      return;
    }
    if (moves.length === 0) return;
    setBusy(true);
    setNotice(null);
    try {
      await aegis.assets.proposeRecovery(moves);
      setSelectedIds(new Set());
      onReviewBatches();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleMoveToPicker = async (items: ForgottenAsset[]) => {
    if (!desktop) {
      setNotice("Preview mode: folder recovery batches run in the desktop app.");
      return;
    }
    if (items.length === 0) return;
    try {
      const dir = await openDialog({ directory: true, multiple: false });
      if (typeof dir !== "string" || !dir) return;
      await proposeMoves(
        items.map((it) => ({
          file_id: it.file_id,
          dest_dir: dir,
          reason: `Recover forgotten asset \`${it.filename}\` to ${dir}`,
        }))
      );
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const handleDismiss = async (item: ForgottenAsset, restore: boolean) => {
    if (!desktop) {
      setNotice("Preview mode: dismissals persist in the encrypted desktop database.");
      return;
    }
    setBusy(true);
    try {
      if (restore) {
        await aegis.assets.undismiss(item.file_id);
      } else {
        await aegis.assets.dismiss(item.file_id, "owner_dismissed");
      }
      await loadScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleInspect = async (item: ForgottenAsset) => {
    if (!desktop) {
      setNotice(`Preview path: ${item.path}`);
      return;
    }
    try {
      setInspected(await aegis.file.get(item.file_id));
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const handleCopyPath = async (path: string) => {
    try {
      await navigator.clipboard.writeText(path);
      setCopiedPath(path);
      setTimeout(() => setCopiedPath((prev) => (prev === path ? null : prev)), 1800);
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
              <Compass className="h-4 w-4 text-emerald-400" aria-hidden="true" />
              Forgotten Assets
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Finds 80+ design, RAW photo, 3D, font, video, audio, and document formats buried in clutter, resting on unplugged drives, or stranded outside matching projects.
            </p>
          </div>
          <button
            type="button"
            disabled={loading || busy}
            onClick={() => void loadScan()}
            className="rounded-md border border-slate-700 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40"
          >
            {loading ? "Scanning…" : "Rescan catalog"}
          </button>
        </div>

        {report && (
          <div className="mt-4 grid grid-cols-2 gap-2.5 sm:grid-cols-5">
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Forgotten assets</p>
              <p className="mt-0.5 text-base font-semibold text-slate-100">
                {report.total_count.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">{formatBytes(report.total_bytes)}</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Matches project/set</p>
              <p className="mt-0.5 text-base font-semibold text-emerald-300">
                {report.stranded_count.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">One-click recovery</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Buried in clutter</p>
              <p className="mt-0.5 text-base font-semibold text-amber-300">
                {report.buried_count.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Downloads / Old copies</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">On unplugged drives</p>
              <p className="mt-0.5 text-base font-semibold text-sky-300">
                {report.offline_count.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Plug in to move</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Only known copy</p>
              <p className="mt-0.5 text-base font-semibold text-rose-300">
                {report.only_copy_count.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Needs safe placement</p>
            </div>
          </div>
        )}

        <form onSubmit={handleQuerySubmit} className="mt-4 flex flex-wrap gap-2">
          <div className="relative min-w-[220px] flex-1">
            <Search className="pointer-events-none absolute left-2.5 top-2.5 h-3.5 w-3.5 text-slate-500" />
            <input
              type="text"
              value={queryInput}
              onChange={(e) => setQueryInput(e.target.value)}
              placeholder="Filter by filename, project name, camera, color hex, or hybrid text…"
              className="w-full rounded-md border border-slate-700 bg-slate-950 py-1.5 pl-8 pr-3 text-xs text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
            />
          </div>
          <button
            type="submit"
            disabled={loading}
            className="rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
          >
            Search assets
          </button>
          {appliedQuery && (
            <button
              type="button"
              onClick={() => {
                setQueryInput("");
                setAppliedQuery("");
              }}
              className="rounded-md border border-slate-700 px-2.5 py-1.5 text-xs text-slate-300 hover:bg-slate-800"
            >
              Clear query
            </button>
          )}
        </form>

        <div className="mt-3 flex flex-wrap items-center gap-2 text-xs">
          <select
            aria-label="Dormancy threshold"
            value={minDormancyDays}
            onChange={(e) => setMinDormancyDays(Number(e.target.value))}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-200"
          >
            {DORMANCY_CHOICES.map((c) => (
              <option key={c.days} value={c.days}>
                {c.label}
              </option>
            ))}
          </select>

          <select
            aria-label="Drive status filter"
            value={driveStatus}
            onChange={(e) => setDriveStatus(e.target.value as "all" | "online" | "offline")}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-200"
          >
            <option value="all">All drives</option>
            <option value="online">Online drives only</option>
            <option value="offline">Unplugged drives only</option>
          </select>

          {report && report.by_drive.length > 1 && (
            <select
              aria-label="Filter by drive"
              value={volumeId}
              onChange={(e) => setVolumeId(e.target.value)}
              className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-200"
            >
              <option value="">Every volume</option>
              {report.by_drive.map((d) => (
                <option key={d.volume_id} value={d.volume_id}>
                  {d.label} ({d.count}){d.online ? "" : " · offline"}
                </option>
              ))}
            </select>
          )}

          <label className="ml-auto inline-flex cursor-pointer items-center gap-1.5 text-slate-300">
            <input
              type="checkbox"
              checked={includeDismissed}
              onChange={(e) => setIncludeDismissed(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950 text-emerald-500"
            />
            Show dismissed ({report?.dismissed_count ?? 0})
          </label>
        </div>

        {report && report.by_family.length > 0 && (
          <div className="mt-3 flex flex-wrap gap-1.5">
            <button
              type="button"
              onClick={() => setFamilyFilter(null)}
              className={`rounded-full px-2.5 py-0.5 text-xs ${
                familyFilter === null
                  ? "bg-emerald-600 font-semibold text-white"
                  : "border border-slate-700 bg-slate-950 text-slate-300 hover:bg-slate-800"
              }`}
            >
              All formats ({report.total_count})
            </button>
            {report.by_family.map((f) => (
              <button
                key={f.family}
                type="button"
                onClick={() => setFamilyFilter((prev) => (prev === f.family ? null : f.family))}
                className={`rounded-full px-2.5 py-0.5 text-xs ${
                  familyFilter === f.family
                    ? "bg-emerald-600 font-semibold text-white"
                    : "border border-slate-700 bg-slate-950 text-slate-300 hover:bg-slate-800"
                }`}
              >
                {f.label} ({f.count} · {formatBytes(f.total_bytes)})
              </button>
            ))}
          </div>
        )}

        {notice && (
          <p className="mt-3 rounded border border-slate-800 bg-slate-950 px-3 py-1.5 text-xs text-amber-300">
            {notice}
          </p>
        )}
      </div>

      {selectedOnlineItems.length > 0 && (
        <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-emerald-800/60 bg-emerald-950/40 px-4 py-2.5 text-xs">
          <span className="font-medium text-emerald-200">
            {selectedOnlineItems.length} online asset(s) selected
          </span>
          <div className="flex flex-wrap gap-2">
            {selectedMatchedItems.length > 0 && (
              <button
                type="button"
                disabled={busy}
                onClick={() =>
                  void proposeMoves(
                    selectedMatchedItems.map((it) => ({
                      file_id: it.file_id,
                      dest_dir: it.suggested_destination!,
                      reason: `Reunite \`${it.filename}\` with ${it.matched_collection_name ?? "project"}`,
                    }))
                  )
                }
                className="inline-flex items-center gap-1 rounded bg-emerald-600 px-3 py-1 font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
              >
                <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
                Recover {selectedMatchedItems.length} to matched projects
              </button>
            )}
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleMoveToPicker(selectedOnlineItems)}
              className="inline-flex items-center gap-1 rounded border border-slate-600 bg-slate-900 px-3 py-1 font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40"
            >
              <FolderInput className="h-3.5 w-3.5" aria-hidden="true" />
              Move selected to folder…
            </button>
          </div>
        </div>
      )}

      {inspected && (
        <div className="rounded-xl border border-emerald-800/60 bg-slate-900/70 p-4 text-xs">
          <div className="flex items-start justify-between gap-2">
            <div>
              <h3 className="font-semibold text-emerald-300">{inspected.name}</h3>
              <p className="break-all font-mono text-slate-400">{inspected.path}</p>
            </div>
            <button
              type="button"
              onClick={() => setInspected(null)}
              className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200"
              aria-label="Close asset inspection"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <div className="mt-2 flex flex-wrap gap-3 text-slate-400">
            <span>Size: {formatBytes(inspected.size)}</span>
            <span>State: {inspected.state}</span>
            <span>Sensitivity: {inspected.sensitivity}</span>
          </div>
          {inspected.chunks_preview.length > 0 && (
            <div className="mt-2 max-h-40 space-y-1.5 overflow-y-auto rounded border border-slate-800 bg-slate-950 p-2.5 text-slate-300">
              {inspected.chunks_preview.map((ch, idx) => (
                <p key={idx} className="whitespace-pre-wrap">
                  {ch}
                </p>
              ))}
            </div>
          )}
        </div>
      )}

      {displayedItems.length === 0 ? (
        <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-6 text-center text-xs text-slate-400">
          {loading
            ? "Scanning catalog for forgotten assets…"
            : "No forgotten assets match the current filters."}
        </div>
      ) : (
        <div className="space-y-2.5">
          {displayedItems.map((item) => {
            const isSelected = selectedIds.has(item.file_id);
            return (
              <article
                key={item.file_id}
                className="rounded-xl border border-slate-800 bg-slate-900/50 p-3.5 text-xs"
              >
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div className="flex min-w-0 flex-1 items-start gap-2.5">
                    <input
                      type="checkbox"
                      aria-label={`Select ${item.filename}`}
                      disabled={!item.drive_online || item.dismissed}
                      checked={isSelected}
                      onChange={() => toggleSelect(item.file_id)}
                      className="mt-1 rounded border-slate-700 bg-slate-950 text-emerald-500 disabled:opacity-30"
                    />
                    <div className="min-w-0 flex-1 space-y-1">
                      <div className="flex flex-wrap items-center gap-1.5">
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-[10px] uppercase text-emerald-300">
                          {item.ext || item.family}
                        </span>
                        {item.dominant_color && (
                          <span
                            className="inline-flex items-center gap-1 rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 font-mono text-[10px] text-slate-300"
                            title={`Dominant accent ${item.dominant_color}`}
                          >
                            <span
                              className="inline-block h-2.5 w-2.5 rounded-full border border-white/20"
                              style={{ backgroundColor: item.dominant_color }}
                            />
                            {item.dominant_color}
                          </span>
                        )}
                        <h3 className="truncate font-semibold text-slate-100">{item.filename}</h3>
                        <span className="text-slate-400">{formatBytes(item.size_bytes)}</span>
                        <span className="text-slate-500">· {item.age_days}d untouched</span>
                      </div>

                      <p className="truncate font-mono text-[11px] text-slate-500" title={item.path}>
                        {item.path}
                      </p>

                      <p className="text-slate-300">{item.summary}</p>

                      <div className="flex flex-wrap items-center gap-1.5 pt-0.5">
                        <span
                          className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] ${
                            item.drive_online
                              ? "border border-emerald-800/60 bg-emerald-950/50 text-emerald-300"
                              : "border border-sky-800/60 bg-sky-950/50 text-sky-300"
                          }`}
                        >
                          {item.drive_online ? (
                            <HardDrive className="h-2.5 w-2.5" aria-hidden="true" />
                          ) : (
                            <Usb className="h-2.5 w-2.5" aria-hidden="true" />
                          )}
                          {item.drive_label}
                          {!item.drive_online ? " (offline)" : ""}
                        </span>

                        {item.only_copy && (
                          <span className="rounded border border-rose-800/60 bg-rose-950/50 px-1.5 py-0.5 text-[10px] text-rose-200">
                            Only copy
                          </span>
                        )}

                        {item.matched_collection_name && (
                          <span className="rounded border border-emerald-800/60 bg-emerald-950/40 px-1.5 py-0.5 text-[10px] text-emerald-200">
                            Matches {item.matched_collection_name}
                          </span>
                        )}

                        {item.width && item.height && (
                          <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-[10px] text-slate-300">
                            {item.width}x{item.height}
                            {item.aspect_label ? ` (${item.aspect_label})` : ""}
                          </span>
                        )}

                        {item.camera && (
                          <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-300">
                            {item.camera}
                          </span>
                        )}

                        {(item.artist || item.album) && (
                          <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-300">
                            {[item.artist, item.album].filter(Boolean).join(" — ")}
                          </span>
                        )}
                      </div>

                      {item.snippet && (
                        <p className="line-clamp-2 rounded border border-slate-800/80 bg-slate-950/60 px-2 py-1 text-[11px] text-slate-400">
                          {item.snippet}
                        </p>
                      )}
                    </div>
                  </div>

                  <div className="flex flex-wrap items-center gap-1.5">
                    {item.drive_online ? (
                      <>
                        {item.suggested_destination && !item.dismissed && (
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() =>
                              void proposeMoves([
                                {
                                  file_id: item.file_id,
                                  dest_dir: item.suggested_destination!,
                                  reason: `Recover \`${item.filename}\` to ${item.matched_collection_name ?? "collection"}`,
                                },
                              ])
                            }
                            className="inline-flex items-center gap-1 rounded bg-emerald-600 px-2.5 py-1 text-[11px] font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                          >
                            <ArchiveRestore className="h-3 w-3" aria-hidden="true" />
                            Recover to {item.matched_collection_name ?? "project"}
                          </button>
                        )}
                        {!item.dismissed && (
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => void handleMoveToPicker([item])}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 px-2.5 py-1 text-[11px] text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                          >
                            <FolderInput className="h-3 w-3" aria-hidden="true" />
                            Move to folder…
                          </button>
                        )}
                      </>
                    ) : (
                      <span className="rounded border border-sky-800/50 bg-sky-950/30 px-2 py-1 text-[11px] text-sky-300">
                        Plug in {item.drive_label} to move
                      </span>
                    )}

                    <button
                      type="button"
                      onClick={() => void handleInspect(item)}
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800"
                    >
                      <FileSearch className="h-3 w-3" aria-hidden="true" /> Inspect
                    </button>

                    <button
                      type="button"
                      onClick={() => void handleCopyPath(item.path)}
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800"
                    >
                      {copiedPath === item.path ? (
                        <>
                          <Check className="h-3 w-3 text-emerald-400" aria-hidden="true" /> Copied
                        </>
                      ) : (
                        <>
                          <Copy className="h-3 w-3" aria-hidden="true" /> Copy path
                        </>
                      )}
                    </button>

                    <button
                      type="button"
                      disabled={busy}
                      onClick={() => void handleDismiss(item, item.dismissed)}
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-400 hover:bg-slate-800 hover:text-slate-200 disabled:opacity-40"
                    >
                      {item.dismissed ? (
                        <>
                          <RotateCcw className="h-3 w-3" aria-hidden="true" /> Restore
                        </>
                      ) : (
                        <>
                          <EyeOff className="h-3 w-3" aria-hidden="true" /> Dismiss
                        </>
                      )}
                    </button>
                  </div>
                </div>
              </article>
            );
          })}
        </div>
      )}
    </div>
  );
}
