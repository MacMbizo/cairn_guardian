import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  Archive,
  Check,
  Columns2,
  Copy,
  EyeOff,
  FileSearch,
  FolderOpen,
  GitBranch,
  History,
  Pin,
  RotateCcw,
  Rows3,
  Search,
  ShieldAlert,
  Sparkles,
  Trash2,
  Usb,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type DiffHunk,
  type DiffLine,
  type FileDetail,
  type VersionChain,
  type VersionDiffReport,
  type VersionDiffSummary,
  type VersionLinkReason,
  type VersionNode,
  type VersionScanReport,
} from "../bindings/client";
import { formatBytes } from "../format";
import { PREVIEW_VERSION_DIFF, PREVIEW_VERSION_REPORT } from "./fixtures/versions";

const SIGNAL_FILTERS: { id: VersionLinkReason | null; label: string }[] = [
  { id: null, label: "All chains" },
  { id: "filename_pattern", label: "Filename" },
  { id: "versions_folder", label: "Versions\\" },
  { id: "file_history", label: "History" },
  { id: "semantic_similarity", label: "Semantic (>=92%)" },
];

function signalLabel(r: VersionLinkReason): string {
  switch (r) {
    case "filename_pattern":
      return "Filename";
    case "versions_folder":
      return "Versions\\";
    case "file_history":
      return "History";
    case "semantic_similarity":
      return "Semantic";
  }
}

function formatDateTime(ms: number): string {
  if (!ms) return "Unknown";
  return new Date(ms).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatTimeDelta(deltaMs: number | null): string | null {
  if (deltaMs === null) return null;
  const sign = deltaMs >= 0 ? "+" : "-";
  const absSec = Math.floor(Math.abs(deltaMs) / 1000);
  if (absSec < 60) return `${sign}${absSec}s`;
  const mins = Math.floor(absSec / 60);
  if (mins < 60) return `${sign}${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 48) return `${sign}${hrs}h`;
  const days = Math.floor(hrs / 24);
  return `${sign}${days}d`;
}

function formatSizeDelta(deltaBytes: number | null): string | null {
  if (deltaBytes === null) return null;
  if (deltaBytes === 0) return "0 B";
  const sign = deltaBytes > 0 ? "+" : "-";
  return `${sign}${formatBytes(Math.abs(deltaBytes))}`;
}

interface PairedDiffRow {
  left: DiffLine | null;
  right: DiffLine | null;
}

function pairHunkLines(hunk: DiffHunk): PairedDiffRow[] {
  const rows: PairedDiffRow[] = [];
  const lines = hunk.lines;
  let i = 0;
  while (i < lines.length) {
    const cur = lines[i];
    if (cur.kind === "context") {
      rows.push({ left: cur, right: cur });
      i += 1;
      continue;
    }
    const removed: DiffLine[] = [];
    const added: DiffLine[] = [];
    while (i < lines.length && lines[i].kind !== "context") {
      if (lines[i].kind === "removed") removed.push(lines[i]);
      else added.push(lines[i]);
      i += 1;
    }
    const maxLen = Math.max(removed.length, added.length);
    for (let k = 0; k < maxLen; k++) {
      rows.push({
        left: removed[k] ?? null,
        right: added[k] ?? null,
      });
    }
  }
  return rows;
}

export function VersionTracking({
  desktop,
  onReviewBatches,
}: {
  desktop: boolean;
  onReviewBatches: () => void;
}) {
  const [report, setReport] = useState<VersionScanReport | null>(
    desktop ? null : PREVIEW_VERSION_REPORT
  );
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const [queryInput, setQueryInput] = useState("");
  const [appliedQuery, setAppliedQuery] = useState("");
  const [signalFilter, setSignalFilter] = useState<VersionLinkReason | null>(null);
  const [onlyWithDupes, setOnlyWithDupes] = useState(false);
  const [includeDismissed, setIncludeDismissed] = useState(false);

  const [selectedChainKey, setSelectedChainKey] = useState<string | null>(null);
  const [customArchiveDir, setCustomArchiveDir] = useState<string | null>(null);
  const [selectedArchiveIds, setSelectedArchiveIds] = useState<Set<string>>(new Set());

  const [olderId, setOlderId] = useState<string>("");
  const [newerId, setNewerId] = useState<string>("");
  const [diffMode, setDiffMode] = useState<"side" | "unified">("side");
  const [diffReport, setDiffReport] = useState<VersionDiffReport | null>(
    desktop ? null : PREVIEW_VERSION_DIFF
  );
  const [diffLoading, setDiffLoading] = useState(false);
  const [llmSummary, setLlmSummary] = useState<VersionDiffSummary | null>(
    desktop ? null : PREVIEW_VERSION_DIFF.cached_llm_summary
  );
  const [llmBusy, setLlmBusy] = useState(false);

  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);

  const loadScan = useCallback(async () => {
    if (!desktop) {
      setReport(PREVIEW_VERSION_REPORT);
      return;
    }
    setLoading(true);
    setNotice(null);
    try {
      const next = await aegis.versions.scan({
        query: appliedQuery || null,
        signal: signalFilter,
        only_with_dupes: onlyWithDupes,
        include_dismissed: includeDismissed,
        limit: 120,
      });
      setReport(next);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [desktop, appliedQuery, signalFilter, onlyWithDupes, includeDismissed]);

  useEffect(() => {
    void loadScan();
  }, [loadScan]);

  const displayedChains = useMemo(() => {
    if (!report) return [];
    if (desktop) return report.chains;
    return report.chains.filter((ch) => {
      if (signalFilter && !ch.link_reasons.includes(signalFilter)) return false;
      if (onlyWithDupes && ch.exact_duplicate_count === 0) return false;
      if (!includeDismissed && ch.dismissed) return false;
      if (appliedQuery) {
        const q = appliedQuery.toLowerCase();
        const hay = `${ch.title} ${ch.anchor_dir} ${ch.collection_name ?? ""} ${ch.nodes
          .map((n) => `${n.filename} ${n.path}`)
          .join(" ")}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [desktop, report, signalFilter, onlyWithDupes, includeDismissed, appliedQuery]);

  const selectedChain: VersionChain | null = useMemo(() => {
    if (displayedChains.length === 0) return null;
    if (selectedChainKey) {
      const found = displayedChains.find((c) => c.chain_key === selectedChainKey);
      if (found) return found;
    }
    return displayedChains[0];
  }, [displayedChains, selectedChainKey]);

  // Reset diff selectors and archive defaults whenever active chain changes.
  useEffect(() => {
    if (!selectedChain) {
      setOlderId("");
      setNewerId("");
      setSelectedArchiveIds(new Set());
      setCustomArchiveDir(null);
      return;
    }
    setCustomArchiveDir(null);
    const nodes = selectedChain.nodes;
    const defaultArchive = new Set(
      nodes
        .filter((n) => !n.is_latest && !n.in_versions_folder && n.drive_online)
        .map((n) => n.file_id)
    );
    setSelectedArchiveIds(defaultArchive);

    if (nodes.length >= 2) {
      const targetNewer =
        nodes.find((n) => n.file_id === selectedChain.latest_file_id) ??
        nodes[nodes.length - 1];
      const candidatesOlder = nodes.filter((n) => n.file_id !== targetNewer.file_id);
      const targetOlder = candidatesOlder[candidatesOlder.length - 1] ?? nodes[0];
      setOlderId(targetOlder.file_id);
      setNewerId(targetNewer.file_id);
    }
  }, [selectedChain]);

  // Load pair diff whenever olderId / newerId change.
  useEffect(() => {
    if (!selectedChain || !olderId || !newerId || olderId === newerId) {
      if (!desktop && selectedChain?.chain_key === "preview-chain-governance") {
        setDiffReport(PREVIEW_VERSION_DIFF);
        setLlmSummary(PREVIEW_VERSION_DIFF.cached_llm_summary);
      } else {
        setDiffReport(null);
        setLlmSummary(null);
      }
      return;
    }
    if (!desktop) {
      if (
        olderId === PREVIEW_VERSION_DIFF.older_file_id &&
        newerId === PREVIEW_VERSION_DIFF.newer_file_id
      ) {
        setDiffReport(PREVIEW_VERSION_DIFF);
        setLlmSummary(PREVIEW_VERSION_DIFF.cached_llm_summary);
      } else {
        const o = selectedChain.nodes.find((n) => n.file_id === olderId);
        const nw = selectedChain.nodes.find((n) => n.file_id === newerId);
        if (o && nw) {
          setDiffReport({
            older_file_id: o.file_id,
            newer_file_id: nw.file_id,
            older_filename: o.filename,
            newer_filename: nw.filename,
            older_path: o.path,
            newer_path: nw.path,
            older_size_bytes: o.size_bytes,
            newer_size_bytes: nw.size_bytes,
            older_mtime: o.mtime,
            newer_mtime: nw.mtime,
            size_delta_bytes: nw.size_bytes - o.size_bytes,
            time_delta_ms: nw.mtime - o.mtime,
            identical_bytes: o.is_exact_duplicate && o.duplicate_of_file_id === nw.file_id,
            diff_kind: o.ext === "psd" ? "binary_metadata" : "text",
            redacted_sensitive: false,
            truncated: false,
            added_lines: 2,
            removed_lines: 1,
            unchanged_lines: 3,
            semantic_similarity: nw.semantic_similarity_prev,
            conceptual_badges: o.ext === "psd" ? ["Binary / media revision"] : ["Preview diff"],
            deterministic_summary:
              o.ext === "psd"
                ? `Canvas asset grew by ${formatBytes(Math.max(0, nw.size_bytes - o.size_bytes))}.`
                : "Select Architecture_Spec_v2 (1).md -> FINAL to view rich sample diff.",
            hunks: [],
            cached_llm_summary: null,
          });
          setLlmSummary(null);
        }
      }
      return;
    }

    let cancelled = false;
    setDiffLoading(true);
    setLlmSummary(null);
    void aegis.versions
      .diff(olderId, newerId)
      .then((rep) => {
        if (!cancelled) {
          setDiffReport(rep);
          setLlmSummary(rep.cached_llm_summary ?? null);
        }
      })
      .catch((err) => {
        if (!cancelled) {
          setNotice(errorMessage(err));
          setDiffReport(null);
        }
      })
      .finally(() => {
        if (!cancelled) setDiffLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, [desktop, selectedChain, olderId, newerId]);

  const handleQuerySubmit = (e: FormEvent) => {
    e.preventDefault();
    setAppliedQuery(queryInput.trim());
  };

  const toggleArchiveNode = (fileId: string) => {
    setSelectedArchiveIds((prev) => {
      const next = new Set(prev);
      if (next.has(fileId)) next.delete(fileId);
      else next.add(fileId);
      return next;
    });
  };

  const handlePickArchiveDir = async () => {
    if (!desktop) {
      setNotice("Preview mode: folder selection runs in the Tauri desktop app.");
      return;
    }
    try {
      const dir = await openDialog({ directory: true, multiple: false });
      if (typeof dir === "string" && dir) {
        setCustomArchiveDir(dir);
      }
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const handleProposeArchive = async () => {
    if (!selectedChain) return;
    if (!desktop) {
      setNotice("Preview mode: launch the desktop app to stage Version archive moves in Simulator.");
      return;
    }
    const ids = Array.from(selectedArchiveIds);
    const destDir = customArchiveDir ?? selectedChain.suggested_versions_dir;
    if (ids.length === 0) return;
    setBusy(true);
    setNotice(null);
    try {
      await aegis.versions.proposeArchive(ids, destDir);
      onReviewBatches();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const duplicateGroup = useMemo(() => {
    if (!selectedChain) return null;
    const dupNodes = selectedChain.nodes.filter((n) => n.is_exact_duplicate && n.drive_online);
    if (dupNodes.length === 0) return null;
    const keeperId =
      dupNodes.find((n) => n.duplicate_of_file_id === selectedChain.latest_file_id)
        ?.duplicate_of_file_id ??
      dupNodes[0].duplicate_of_file_id ??
      selectedChain.latest_file_id;
    const sameKeeperIds = dupNodes
      .filter((n) => (n.duplicate_of_file_id ?? selectedChain.latest_file_id) === keeperId)
      .map((n) => n.file_id);
    return {
      keeperId,
      duplicateIds: sameKeeperIds,
    };
  }, [selectedChain]);

  const handleProposeQuarantineDupes = async () => {
    if (!selectedChain || !duplicateGroup) return;
    if (!desktop) {
      setNotice("Preview mode: quarantine proposals run in the Tauri desktop Simulator.");
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      await aegis.versions.proposeQuarantineDupes(
        duplicateGroup.keeperId,
        duplicateGroup.duplicateIds
      );
      onReviewBatches();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handlePinLatest = async (node: VersionNode) => {
    if (!selectedChain) return;
    if (!desktop) {
      setNotice(`Preview mode: pinned "${node.filename}" locally.`);
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      await aegis.versions.pinLatest(selectedChain.chain_key, node.file_id);
      await loadScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleDismissToggle = async (chain: VersionChain) => {
    if (!desktop) {
      setNotice("Preview mode: chain dismissals persist in the encrypted desktop database.");
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      if (chain.dismissed) {
        await aegis.versions.undismiss(chain.chain_key);
      } else {
        await aegis.versions.dismiss(chain.chain_key);
      }
      await loadScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleExplainWithLlm = async () => {
    if (!olderId || !newerId || olderId === newerId) return;
    if (!desktop) {
      setNotice("Preview mode: on-demand local GGUF summaries run in the desktop app.");
      return;
    }
    setLlmBusy(true);
    setNotice(null);
    try {
      const res = await aegis.versions.summarizeDiff(olderId, newerId);
      setLlmSummary(res);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLlmBusy(false);
    }
  };

  const handleInspect = async (node: VersionNode) => {
    if (!desktop) {
      setNotice(`Preview path: ${node.path}`);
      return;
    }
    try {
      setInspected(await aegis.file.get(node.file_id));
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
      <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
              <GitBranch className="h-4 w-4 text-emerald-400" aria-hidden="true" />
              Version Tracking
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Groups filename iterations (`_v1` / `_FINAL` / dates), project `Versions\` folders,
              move lineage (`file_history`), and semantically similar drafts (`&gt;= 92%`) — never
              auto-deleting near-duplicates.
            </p>
          </div>
          <button
            type="button"
            disabled={loading || busy}
            onClick={() => void loadScan()}
            className="rounded-md border border-slate-700 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
          >
            {loading ? "Scanning…" : "Rescan chains"}
          </button>
        </div>

        {report && (
          <div className="mt-4 grid grid-cols-2 gap-2.5 sm:grid-cols-4">
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-xs text-slate-400">Version chains</p>
              <p className="mt-0.5 text-base font-semibold text-slate-100">
                {report.total_chains.toLocaleString()}
              </p>
              <p className="text-xs text-slate-500">
                {report.versions_folder_chains_count} use Versions\
              </p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-xs text-slate-400">Tracked revisions</p>
              <p className="mt-0.5 text-base font-semibold text-emerald-300">
                {report.total_versions.toLocaleString()}
              </p>
              <p className="text-xs text-slate-500">Across non-backup drives</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-xs text-slate-400">Semantic chains</p>
              <p className="mt-0.5 text-base font-semibold text-violet-300">
                {report.semantic_chains_count.toLocaleString()}
              </p>
              <p className="text-xs text-slate-500">&gt;= 92% vector similarity</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-xs text-slate-400">Exact dupes in chains</p>
              <p className="mt-0.5 text-base font-semibold text-amber-300">
                {report.chains_with_dupes.toLocaleString()} chains
              </p>
              <p className="text-xs text-slate-500">
                {formatBytes(report.reclaimable_duplicate_bytes)} reclaimable (BLAKE3)
              </p>
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
              placeholder="Filter by filename, project name, or folder path…"
              className="w-full rounded-md border border-slate-700 bg-slate-950 py-1.5 pl-8 pr-3 text-xs text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
            />
          </div>
          <button
            type="submit"
            disabled={loading}
            className="rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
          >
            Filter
          </button>
          {appliedQuery && (
            <button
              type="button"
              onClick={() => {
                setQueryInput("");
                setAppliedQuery("");
              }}
              className="rounded-md border border-slate-700 px-2.5 py-1.5 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
            >
              Clear
            </button>
          )}
        </form>

        <div className="mt-3 flex flex-wrap items-center gap-2">
          {SIGNAL_FILTERS.map((sf) => (
            <button
              key={sf.label}
              type="button"
              onClick={() => setSignalFilter(sf.id)}
              className={`rounded-full px-2.5 py-1 text-xs font-medium transition ${
                signalFilter === sf.id
                  ? "bg-emerald-500/20 text-emerald-300 ring-1 ring-emerald-500/40"
                  : "bg-slate-800/80 text-slate-300 hover:bg-slate-800"
              }`}
            >
              {sf.label}
            </button>
          ))}
          <label className="ml-2 flex items-center gap-1.5 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={onlyWithDupes}
              onChange={(e) => setOnlyWithDupes(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950"
            />
            Has exact duplicates
          </label>
          <label className="ml-2 flex items-center gap-1.5 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={includeDismissed}
              onChange={(e) => setIncludeDismissed(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950"
            />
            Show dismissed{report && report.dismissed_count > 0 ? ` (${report.dismissed_count})` : ""}
          </label>
        </div>
      </div>

      {notice && (
        <div className="flex items-center justify-between rounded-lg border border-amber-800/60 bg-amber-950/40 px-3 py-2 text-xs text-amber-200">
          <span>{notice}</span>
          <button
            type="button"
            onClick={() => setNotice(null)}
            className="text-amber-300 hover:text-amber-100"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

      {displayedChains.length === 0 ? (
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-8 text-center text-xs text-slate-400">
          {loading
            ? "Scanning catalogued files for semantic version chains…"
            : "No multi-version file chains matched the current filters."}
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-[340px_1fr]">
          {/* Left pane: chain selector list */}
          <div className="space-y-2 lg:max-h-[780px] lg:overflow-y-auto lg:pr-1">
            {displayedChains.map((ch) => {
              const active = selectedChain?.chain_key === ch.chain_key;
              return (
                <button
                  key={ch.chain_key}
                  type="button"
                  onClick={() => setSelectedChainKey(ch.chain_key)}
                  className={`w-full rounded-xl border p-3 text-left transition ${
                    active
                      ? "border-emerald-500/50 bg-emerald-950/20"
                      : "border-slate-800 bg-slate-900/60 hover:border-slate-700 hover:bg-slate-900/60"
                  }`}
                >
                  <div className="flex items-start justify-between gap-2">
                    <span className="truncate text-xs font-semibold text-slate-100">
                      {ch.title}
                    </span>
                    <span className="shrink-0 rounded bg-slate-800 px-1.5 py-0.5 text-xs font-medium text-slate-300">
                      {ch.version_count} rev
                    </span>
                  </div>
                  <p className="mt-1 truncate text-xs text-slate-400">{ch.anchor_dir}</p>
                  <div className="mt-2 flex flex-wrap gap-1">
                    {ch.collection_name && (
                      <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-xs text-emerald-300">
                        {ch.collection_name}
                      </span>
                    )}
                    {!ch.drive_online && (
                      <span className="inline-flex items-center gap-1 rounded bg-sky-500/15 px-1.5 py-0.5 text-xs text-sky-300">
                        <Usb className="h-2.5 w-2.5" />
                        Offline
                      </span>
                    )}
                    {ch.exact_duplicate_count > 0 && (
                      <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-xs text-amber-300">
                        {ch.exact_duplicate_count} exact dup
                      </span>
                    )}
                    {ch.has_semantic_links && (
                      <span className="rounded bg-violet-500/15 px-1.5 py-0.5 text-xs text-violet-300">
                        Semantic
                      </span>
                    )}
                    {ch.has_versions_folder && (
                      <span className="rounded bg-sky-500/15 px-1.5 py-0.5 text-xs text-sky-300">
                        Versions\
                      </span>
                    )}
                    {ch.has_file_history && (
                      <span className="rounded bg-teal-500/15 px-1.5 py-0.5 text-xs text-teal-300">
                        History
                      </span>
                    )}
                  </div>
                </button>
              );
            })}
          </div>

          {/* Right pane: timeline + reversible actions + semantic diff */}
          {selectedChain && (
            <div className="space-y-4">
              {/* Header & reversible actions */}
              <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                <div className="flex flex-wrap items-start justify-between gap-3">
                  <div>
                    <div className="flex flex-wrap items-center gap-2">
                      <h3 className="text-sm font-semibold text-slate-100">
                        {selectedChain.title}
                      </h3>
                      <span className="rounded bg-slate-800 px-2 py-0.5 text-xs text-slate-300">
                        {selectedChain.drive_label}
                      </span>
                      {!selectedChain.drive_online && (
                        <span className="rounded bg-sky-500/15 px-2 py-0.5 text-xs text-sky-300">
                          Plug drive in to move or diff unindexed files
                        </span>
                      )}
                      {selectedChain.has_sensitive && (
                        <span className="inline-flex items-center gap-1 rounded bg-rose-500/15 px-2 py-0.5 text-xs text-rose-300">
                          <ShieldAlert className="h-3 w-3" />
                          Contains a sensitive file
                        </span>
                      )}
                    </div>
                    <p className="mt-1 text-xs text-slate-400">{selectedChain.summary}</p>
                  </div>

                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void handleDismissToggle(selectedChain)}
                    className="inline-flex items-center gap-1 rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                  >
                    {selectedChain.dismissed ? (
                      <>
                        <RotateCcw className="h-3.5 w-3.5" />
                        Restore chain
                      </>
                    ) : (
                      <>
                        <EyeOff className="h-3.5 w-3.5" />
                        Dismiss chain
                      </>
                    )}
                  </button>
                </div>

                {/* Archive & Quarantine Controls */}
                <div className="mt-4 flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3">
                  <div className="flex flex-wrap items-center gap-2 text-xs">
                    <Archive className="h-3.5 w-3.5 text-emerald-400" />
                    <span className="text-slate-400">Archive destination:</span>
                    <code className="max-w-[320px] truncate rounded bg-slate-900 px-2 py-0.5 text-xs text-slate-200">
                      {customArchiveDir ?? selectedChain.suggested_versions_dir}
                    </code>
                    <button
                      type="button"
                      disabled={busy || !selectedChain.drive_online}
                      onClick={() => void handlePickArchiveDir()}
                      className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-0.5 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                    >
                      <FolderOpen className="h-3 w-3" />
                      Choose folder…
                    </button>
                  </div>

                  <div className="flex flex-wrap items-center gap-2">
                    <button
                      type="button"
                      disabled={busy || selectedArchiveIds.size === 0 || !selectedChain.drive_online}
                      onClick={() => void handleProposeArchive()}
                      className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                    >
                      <Archive className="h-3.5 w-3.5" />
                      Archive older to Versions ({selectedArchiveIds.size})
                    </button>

                    {duplicateGroup && (
                      <button
                        type="button"
                        disabled={busy || !selectedChain.drive_online}
                        onClick={() => void handleProposeQuarantineDupes()}
                        className="inline-flex items-center gap-1.5 rounded-md border border-amber-700/70 bg-amber-950/40 px-3 py-1.5 text-xs font-semibold text-amber-200 hover:bg-amber-900/50 disabled:opacity-40"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                        Quarantine exact dupes ({duplicateGroup.duplicateIds.length})
                      </button>
                    )}
                  </div>
                </div>
                <p className="mt-1.5 text-xs text-slate-500">
                  Safety rule (`FR-004.2`): Near-identical semantic revisions (`&gt;= 92%`) are
                  never auto-deleted. Only verified byte-identical `BLAKE3` duplicates can enter
                  30-day quarantine, preserving `MIN_COPIES = 2`.
                </p>
              </div>

              {/* Chronological Version Timeline */}
              <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                <h4 className="mb-3 text-xs font-semibold uppercase tracking-wide text-slate-400">
                  Version Timeline (oldest -&gt; newest)
                </h4>
                <div className="space-y-2">
                  {selectedChain.nodes.map((node, index) => {
                    const sizeDelta = formatSizeDelta(node.size_delta_bytes);
                    const timeDelta = formatTimeDelta(node.time_delta_ms);
                    const canArchive =
                      !node.is_latest && !node.in_versions_folder && node.drive_online;
                    return (
                      <div
                        key={node.file_id}
                        className={`rounded-lg border p-3 ${
                          node.is_latest
                            ? "border-emerald-500/40 bg-emerald-950/15"
                            : "border-slate-800 bg-slate-950/50"
                        }`}
                      >
                        <div className="flex flex-wrap items-start justify-between gap-2">
                          <div className="flex min-w-0 items-start gap-2.5">
                            {canArchive ? (
                              <input
                                type="checkbox"
                                aria-label={`Select ${node.filename} to archive`}
                                checked={selectedArchiveIds.has(node.file_id)}
                                onChange={() => toggleArchiveNode(node.file_id)}
                                className="mt-1 rounded border-slate-700 bg-slate-900"
                              />
                            ) : (
                              <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded bg-slate-800 text-xs font-semibold text-slate-300">
                                {index + 1}
                              </span>
                            )}
                            <div className="min-w-0">
                              <div className="flex flex-wrap items-center gap-1.5">
                                <span className="truncate text-xs font-semibold text-slate-100">
                                  {node.filename}
                                </span>
                                <span className="rounded bg-slate-800 px-1.5 py-0.5 text-xs text-slate-300">
                                  {node.version_label}
                                </span>
                                {node.is_latest && (
                                  <span className="rounded bg-emerald-500/20 px-1.5 py-0.5 text-xs font-semibold text-emerald-300">
                                    Latest
                                  </span>
                                )}
                                {node.is_pinned && (
                                  <span className="rounded bg-indigo-500/20 px-1.5 py-0.5 text-xs text-indigo-300">
                                    Pinned
                                  </span>
                                )}
                                {node.in_versions_folder && (
                                  <span className="rounded bg-sky-500/15 px-1.5 py-0.5 text-xs text-sky-300">
                                    In Versions\
                                  </span>
                                )}
                                {node.is_exact_duplicate && (
                                  <span className="rounded bg-amber-500/15 px-1.5 py-0.5 text-xs text-amber-300">
                                    Exact copy (BLAKE3)
                                  </span>
                                )}
                                {node.semantic_similarity_prev !== null && (
                                  <span className="rounded bg-violet-500/15 px-1.5 py-0.5 text-xs text-violet-300">
                                    {Math.round(node.semantic_similarity_prev * 100)}% similar to
                                    prev
                                  </span>
                                )}
                              </div>

                              <p className="mt-1 truncate text-xs text-slate-400">
                                {node.path}
                              </p>

                              <div className="mt-1 flex flex-wrap items-center gap-3 text-xs text-slate-400">
                                <span>{formatBytes(node.size_bytes)}</span>
                                {sizeDelta && <span className="text-slate-300">({sizeDelta})</span>}
                                <span>{formatDateTime(node.mtime)}</span>
                                {timeDelta && <span className="text-slate-300">({timeDelta})</span>}
                                {node.link_reasons.map((r) => (
                                  <span key={r} className="text-slate-500">
                                    · {signalLabel(r)}
                                  </span>
                                ))}
                              </div>

                              {node.former_paths.length > 0 && (
                                <p className="mt-1 flex items-center gap-1 text-xs text-teal-300">
                                  <History className="h-3 w-3 shrink-0" />
                                  <span className="truncate">
                                    Moved from: {node.former_paths.join(" · ")}
                                  </span>
                                </p>
                              )}
                            </div>
                          </div>

                          <div className="flex shrink-0 flex-wrap items-center gap-1">
                            {!node.is_latest && (
                              <button
                                type="button"
                                disabled={busy}
                                onClick={() => void handlePinLatest(node)}
                                title="Pin as Latest"
                                className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                              >
                                <Pin className="h-3 w-3" />
                                Pin
                              </button>
                            )}
                            <button
                              type="button"
                              onClick={() => void handleInspect(node)}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
                            >
                              <FileSearch className="h-3 w-3" />
                              Inspect
                            </button>
                            <button
                              type="button"
                              onClick={() => void handleCopyPath(node.path)}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
                            >
                              {copiedPath === node.path ? (
                                <Check className="h-3 w-3 text-emerald-400" />
                              ) : (
                                <Copy className="h-3 w-3" />
                              )}
                              {copiedPath === node.path ? "Copied" : "Copy"}
                            </button>
                          </div>
                        </div>
                      </div>
                    );
                  })}
                </div>
              </div>

              {/* Visual Semantic Diff & Conceptual Change Summary */}
              <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
                <div className="flex flex-wrap items-center justify-between gap-3">
                  <div className="flex flex-wrap items-center gap-2 text-xs">
                    <span className="font-semibold text-slate-200">Compare:</span>
                    <select
                      aria-label="Older version"
                      value={olderId}
                      onChange={(e) => setOlderId(e.target.value)}
                      className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100"
                    >
                      {selectedChain.nodes.map((n) => (
                        <option key={n.file_id} value={n.file_id}>
                          {n.filename} ({n.version_label})
                        </option>
                      ))}
                    </select>
                    <span className="text-slate-500">-&gt;</span>
                    <select
                      aria-label="Newer version"
                      value={newerId}
                      onChange={(e) => setNewerId(e.target.value)}
                      className="rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100"
                    >
                      {selectedChain.nodes.map((n) => (
                        <option key={n.file_id} value={n.file_id}>
                          {n.filename} ({n.version_label})
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="flex items-center gap-2">
                    <div className="inline-flex rounded-md border border-slate-700 bg-slate-950 p-0.5 text-xs">
                      <button
                        type="button"
                        onClick={() => setDiffMode("side")}
                        className={`inline-flex items-center gap-1 rounded px-2 py-1 ${
                          diffMode === "side"
                            ? "bg-emerald-500/20 text-emerald-300"
                            : "text-slate-400 hover:text-slate-200"
                        }`}
                      >
                        <Columns2 className="h-3 w-3" />
                        Side-by-side
                      </button>
                      <button
                        type="button"
                        onClick={() => setDiffMode("unified")}
                        className={`inline-flex items-center gap-1 rounded px-2 py-1 ${
                          diffMode === "unified"
                            ? "bg-emerald-500/20 text-emerald-300"
                            : "text-slate-400 hover:text-slate-200"
                        }`}
                      >
                        <Rows3 className="h-3 w-3" />
                        Unified
                      </button>
                    </div>

                    <button
                      type="button"
                      disabled={
                        llmBusy ||
                        !diffReport ||
                        diffReport.redacted_sensitive ||
                        diffReport.identical_bytes ||
                        diffReport.diff_kind !== "text"
                      }
                      onClick={() => void handleExplainWithLlm()}
                      className="inline-flex items-center gap-1.5 rounded-md bg-violet-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-violet-500 disabled:opacity-40"
                    >
                      <Sparkles className="h-3.5 w-3.5" />
                      {llmBusy ? "Summarizing…" : "Explain changes (Local LLM)"}
                    </button>
                  </div>
                </div>

                {olderId === newerId && olderId && (
                  <p className="mt-3 text-xs text-slate-400">
                    Choose two different revisions to view changes.
                  </p>
                )}

                {diffLoading && (
                  <p className="mt-3 text-xs text-slate-400">Computing semantic diff…</p>
                )}

                {diffReport && !diffLoading && (
                  <div className="mt-3 space-y-3">
                    <div className="flex flex-wrap items-center gap-1.5">
                      {diffReport.conceptual_badges.map((badge) => (
                        <span
                          key={badge}
                          className="rounded-full bg-slate-800 px-2.5 py-0.5 text-xs font-medium text-slate-200"
                        >
                          {badge}
                        </span>
                      ))}
                      {diffReport.truncated && (
                        <span className="rounded-full bg-amber-500/15 px-2.5 py-0.5 text-xs text-amber-300">
                          Diff truncated to 1,200 lines
                        </span>
                      )}
                    </div>

                    <div className="rounded-lg border border-slate-800 bg-slate-950/70 p-3 text-xs text-slate-200">
                      <p className="font-medium text-slate-300">Conceptual Delta</p>
                      <p className="mt-1 text-slate-400">{diffReport.deterministic_summary}</p>

                      {llmSummary && (
                        <div className="mt-2.5 border-t border-slate-800 pt-2.5">
                          <div className="flex items-center gap-2">
                            <span className="inline-flex items-center gap-1 text-xs font-semibold text-violet-300">
                              <Sparkles className="h-3 w-3" />
                              Local LLM Summary ({llmSummary.model_name})
                            </span>
                            {llmSummary.cached && (
                              <span className="rounded bg-slate-800 px-1.5 py-0.5 text-xs text-slate-400">
                                Cached
                              </span>
                            )}
                          </div>
                          <p className="mt-1 leading-relaxed text-slate-200">
                            {llmSummary.summary}
                          </p>
                        </div>
                      )}
                    </div>

                    {diffReport.hunks.length > 0 && (
                      <div className="max-h-[420px] overflow-auto rounded-lg border border-slate-800 bg-slate-950 font-mono text-xs">
                        {diffReport.hunks.map((hunk, hIdx) => (
                          <div key={hIdx} className="border-b border-slate-800 last:border-b-0">
                            <div className="bg-slate-900/90 px-3 py-1 text-xs text-sky-300">
                              @@ -{hunk.old_start},{hunk.old_count} +{hunk.new_start},
                              {hunk.new_count} @@
                            </div>

                            {diffMode === "unified" ? (
                              <div>
                                {hunk.lines.map((line, lIdx) => (
                                  <div
                                    key={lIdx}
                                    className={`flex gap-2 px-3 py-0.5 ${
                                      line.kind === "added"
                                        ? "bg-emerald-950/40 text-emerald-200"
                                        : line.kind === "removed"
                                        ? "bg-rose-950/40 text-rose-200"
                                        : "text-slate-400"
                                    }`}
                                  >
                                    <span className="w-8 shrink-0 text-right text-slate-600">
                                      {line.old_line ?? ""}
                                    </span>
                                    <span className="w-8 shrink-0 text-right text-slate-600">
                                      {line.new_line ?? ""}
                                    </span>
                                    <span className="w-3 shrink-0 select-none">
                                      {line.kind === "added"
                                        ? "+"
                                        : line.kind === "removed"
                                        ? "-"
                                        : " "}
                                    </span>
                                    <span className="whitespace-pre-wrap break-all">
                                      {line.text}
                                    </span>
                                  </div>
                                ))}
                              </div>
                            ) : (
                              <div>
                                {pairHunkLines(hunk).map((pair, rIdx) => (
                                  <div
                                    key={rIdx}
                                    className="grid grid-cols-2 divide-x divide-slate-800/80"
                                  >
                                    <div
                                      className={`flex gap-2 px-2.5 py-0.5 ${
                                        pair.left?.kind === "removed"
                                          ? "bg-rose-950/40 text-rose-200"
                                          : "text-slate-400"
                                      }`}
                                    >
                                      <span className="w-7 shrink-0 text-right text-slate-600">
                                        {pair.left?.old_line ?? ""}
                                      </span>
                                      <span className="whitespace-pre-wrap break-all">
                                        {pair.left?.text ?? ""}
                                      </span>
                                    </div>
                                    <div
                                      className={`flex gap-2 px-2.5 py-0.5 ${
                                        pair.right?.kind === "added"
                                          ? "bg-emerald-950/40 text-emerald-200"
                                          : "text-slate-400"
                                      }`}
                                    >
                                      <span className="w-7 shrink-0 text-right text-slate-600">
                                        {pair.right?.new_line ?? ""}
                                      </span>
                                      <span className="whitespace-pre-wrap break-all">
                                        {pair.right?.text ?? ""}
                                      </span>
                                    </div>
                                  </div>
                                ))}
                              </div>
                            )}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {inspected && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/80 p-4">
          <div className="max-h-[80vh] w-full max-w-2xl overflow-y-auto rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
            <div className="flex items-start justify-between gap-2">
              <div>
                <h3 className="text-sm font-semibold text-slate-100">{inspected.name}</h3>
                <p className="mt-0.5 break-all text-xs text-slate-400">{inspected.path}</p>
              </div>
              <button
                type="button"
                onClick={() => setInspected(null)}
                className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="mt-3 flex flex-wrap gap-3 text-xs text-slate-300">
              <span>Size: {formatBytes(inspected.size)}</span>
              <span>State: {inspected.state}</span>
              <span>Sensitivity: {inspected.sensitivity}</span>
              <span>Cloud: {inspected.cloud_state}</span>
            </div>
            <div className="mt-4 space-y-2">
              <p className="text-xs font-medium text-slate-300">Indexed snippet preview</p>
              {inspected.chunks_preview.length === 0 ? (
                <p className="text-xs text-slate-500">
                  No text chunks indexed for this binary or unparsed revision.
                </p>
              ) : (
                inspected.chunks_preview.map((chunk, i) => (
                  <pre
                    key={i}
                    className="whitespace-pre-wrap rounded-lg border border-slate-800 bg-slate-950 p-3 font-mono text-xs text-slate-200"
                  >
                    {chunk}
                  </pre>
                ))
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
