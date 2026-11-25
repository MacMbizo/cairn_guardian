import { useCallback, useEffect, useMemo, useState } from "react";
import {
  AlertTriangle,
  ArchiveRestore,
  CheckCircle2,
  Eye,
  FileSearch,
  FolderCog,
  Hash,
  History,
  Link2,
  Play,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Stethoscope,
  Tags,
  Wrench,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type BrokenLinkItem,
  type DoctorReport,
  type FileDetail,
  type HealBatchSummary,
  type HealPatchPreviewFile,
  type HealScanReport,
  type RulePathIssue,
  type TagWeedCluster,
} from "../bindings/client";
import { formatBytes } from "../format";
import {
  PREVIEW_DOCTOR_REPORT,
  PREVIEW_HEAL_BATCHES,
  PREVIEW_HEAL_DIFFS,
  PREVIEW_HEAL_REPORT,
} from "./fixtures/healing";

type HealSubTab = "links" | "tags" | "rules" | "batches";

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

function statusPill(status: BrokenLinkItem["status"]) {
  switch (status) {
    case "resolved":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-emerald-950/60 px-2 py-0.5 text-[11px] text-emerald-300 border border-emerald-800/60">
          <CheckCircle2 className="h-3 w-3" /> Resolved
        </span>
      );
    case "in_quarantine":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-amber-950/60 px-2 py-0.5 text-[11px] text-amber-300 border border-amber-800/60">
          <ArchiveRestore className="h-3 w-3" /> In Quarantine
        </span>
      );
    case "ambiguous":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-orange-950/60 px-2 py-0.5 text-[11px] text-orange-300 border border-orange-800/60">
          <AlertTriangle className="h-3 w-3" /> Ambiguous
        </span>
      );
    default:
      return (
        <span className="inline-flex items-center gap-1 rounded bg-slate-800 px-2 py-0.5 text-[11px] text-slate-300 border border-slate-700">
          Unresolved
        </span>
      );
  }
}

export function SelfHealing({
  desktop,
  onReviewBatches,
}: {
  desktop: boolean;
  onReviewBatches: () => void;
}) {
  const [subTab, setSubTab] = useState<HealSubTab>("links");
  const [report, setReport] = useState<HealScanReport | null>(
    desktop ? null : PREVIEW_HEAL_REPORT
  );
  const [batches, setBatches] = useState<HealBatchSummary[]>(
    desktop ? [] : PREVIEW_HEAL_BATCHES
  );
  const [doctor, setDoctor] = useState<DoctorReport | null>(
    desktop ? null : PREVIEW_DOCTOR_REPORT
  );

  const [scopeRoot, setScopeRoot] = useState("");
  const [includeSensitive, setIncludeSensitive] = useState(false);
  const [includeDismissed, setIncludeDismissed] = useState(false);
  const [selectedLinks, setSelectedLinks] = useState<Set<string>>(() =>
    desktop
      ? new Set()
      : new Set(
          PREVIEW_HEAL_REPORT.broken_links
            .filter((l) => l.status === "resolved")
            .map((l) => l.issue_key)
        )
  );
  const [selectedTags, setSelectedTags] = useState<Set<string>>(() =>
    desktop
      ? new Set()
      : new Set(
          PREVIEW_HEAL_REPORT.tag_clusters
            .filter((c) => c.patchable_count > 0)
            .map((c) => c.issue_key)
        )
  );

  const [scanning, setScanning] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [staging, setStaging] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [diffs, setDiffs] = useState<HealPatchPreviewFile[] | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const [ruleEdits, setRuleEdits] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<string | null>(null);

  const runScan = useCallback(
    async (sensOverride?: boolean) => {
      if (!desktop) {
        setReport(PREVIEW_HEAL_REPORT);
        setBatches(PREVIEW_HEAL_BATCHES);
        return;
      }
      setScanning(true);
      setNotice(null);
      try {
        const useSens = sensOverride ?? includeSensitive;
        const [res, batchList] = await Promise.all([
          aegis.healing.scan({
            scope_root: scopeRoot.trim() || null,
            include_sensitive: useSens,
            include_dismissed: includeDismissed,
            limit_notes: 500,
          }),
          aegis.healing.recentBatches(20),
        ]);
        setReport(res);
        setBatches(batchList);
        setSelectedLinks(
          new Set(
            res.broken_links
              .filter((l) => l.status === "resolved" && !l.dismissed)
              .map((l) => l.issue_key)
          )
        );
        setSelectedTags(
          new Set(
            res.tag_clusters
              .filter((c) => c.patchable_count > 0 && !c.dismissed)
              .map((c) => c.issue_key)
          )
        );
        setDiffs(null);
      } catch (err) {
        setNotice(errorMessage(err));
      } finally {
        setScanning(false);
      }
    },
    [desktop, includeDismissed, includeSensitive, scopeRoot]
  );

  useEffect(() => {
    void runScan();
  }, [runScan]);

  const healableLinkCount = useMemo(
    () => selectedLinks.size,
    [selectedLinks]
  );
  const patchableTagClusterCount = useMemo(
    () => selectedTags.size,
    [selectedTags]
  );

  const handleToggleSensitive = async () => {
    if (!desktop) {
      setIncludeSensitive((v) => !v);
      return;
    }
    if (!includeSensitive) {
      try {
        await aegis.vault.unlockSensitive();
        setIncludeSensitive(true);
        await runScan(true);
      } catch (err) {
        setNotice(errorMessage(err));
      }
    } else {
      setIncludeSensitive(false);
      await runScan(false);
    }
  };

  const handlePreviewDiffs = async () => {
    const linkIssueKeys = Array.from(selectedLinks);
    const tagIssueKeys = Array.from(selectedTags);
    if (linkIssueKeys.length === 0 && tagIssueKeys.length === 0) {
      setNotice("Select at least one resolved link or patchable tag cluster.");
      return;
    }
    if (!desktop) {
      setDiffs(PREVIEW_HEAL_DIFFS);
      return;
    }
    setPreviewing(true);
    setNotice(null);
    try {
      const res = await aegis.healing.previewPatches({
        linkIssueKeys,
        tagIssueKeys,
        includeSensitive,
      });
      setDiffs(res);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setPreviewing(false);
    }
  };

  const handleStageBatch = async () => {
    const linkIssueKeys = Array.from(selectedLinks);
    const tagIssueKeys = Array.from(selectedTags);
    if (linkIssueKeys.length === 0 && tagIssueKeys.length === 0) {
      setNotice("Select at least one resolved link or patchable tag cluster.");
      return;
    }
    if (!desktop) {
      setNotice(
        "Preview mode: in the desktop app this stages an approvable PatchText batch into the Simulator."
      );
      return;
    }
    setStaging(true);
    setNotice(null);
    try {
      const batchId = await aegis.healing.proposeBatch({
        linkIssueKeys,
        tagIssueKeys,
        includeSensitive,
      });
      setNotice(
        `Self-healing batch ${batchId.slice(0, 8)} is ready to review in the Simulator.`
      );
      setDiffs(null);
      await runScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setStaging(false);
    }
  };

  const handleDismiss = async (issueKey: string, issueKind: string) => {
    if (!desktop) return;
    setBusyId(issueKey);
    try {
      await aegis.healing.dismiss(issueKey, issueKind, "Owner dismissed");
      await runScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleUndismiss = async (issueKey: string) => {
    if (!desktop) return;
    setBusyId(issueKey);
    try {
      await aegis.healing.undismiss(issueKey);
      await runScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleApplyRuleFix = async (issue: RulePathIssue) => {
    const target = (ruleEdits[issue.issue_key] ?? issue.suggested_path ?? "").trim();
    if (!target) {
      setNotice("Provide a replacement path for the rule first.");
      return;
    }
    if (!desktop) {
      setNotice(`Preview mode: would update ${issue.rule_name} (${issue.field}) to ${target}`);
      return;
    }
    setBusyId(issue.issue_key);
    setNotice(null);
    try {
      await aegis.healing.updateRulePath(issue.rule_id, issue.field, target);
      setNotice(`Updated "${issue.rule_name}" (${issue.field}) -> ${target}.`);
      await runScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleRollbackBatch = async (batchId: string) => {
    if (!desktop) return;
    setBusyId(batchId);
    setNotice(null);
    try {
      const rb = await aegis.actions.rollback(batchId);
      setNotice(
        `Rolled back batch ${batchId.slice(0, 8)}: restored ${rb.reverted_count} note(s) (${rb.status}).`
      );
      await runScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleDoctorVerify = async (repair: boolean) => {
    if (!desktop) {
      setDoctor(PREVIEW_DOCTOR_REPORT);
      return;
    }
    setBusyId("doctor");
    setNotice(null);
    try {
      const res = await aegis.healing.runDoctorCheck(repair);
      setDoctor(res);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleInspectFile = async (fileId: string) => {
    if (!desktop) return;
    try {
      const d = await aegis.file.get(fileId);
      setInspected(d);
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const toggleLinkSelect = (key: string) => {
    setSelectedLinks((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const toggleTagSelect = (key: string) => {
    setSelectedTags((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  return (
    <div className="space-y-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <Wrench className="h-5 w-5 text-emerald-400" />
            <h1 className="text-lg font-semibold">Self-Healing Workspace</h1>
            <span className="rounded bg-emerald-950/70 px-2 py-0.5 text-[11px] font-medium text-emerald-300 border border-emerald-800/50">
              You approve every fix · one-click undo
            </span>
          </div>
          <p className="mt-1 text-xs text-slate-400">
            Repairs broken Markdown &amp; wikilinks across external/USN moves, consolidates casing &amp;
            typo tag weeds, catches hex color false positives, and audits Folderless rule paths — every edit
            runs transactionally through the Simulator with TOCTOU verification and one-click undo.
          </p>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            onClick={() => void handleToggleSensitive()}
            className={`inline-flex items-center gap-1.5 rounded-lg border px-2.5 py-1.5 text-xs ${
              includeSensitive
                ? "border-amber-600/70 bg-amber-950/40 text-amber-200"
                : "border-slate-700 bg-slate-900 text-slate-300 hover:bg-slate-800"
            }`}
          >
            {includeSensitive ? (
              <ShieldCheck className="h-3.5 w-3.5 text-amber-400" />
            ) : (
              <ShieldAlert className="h-3.5 w-3.5 text-slate-400" />
            )}
            {includeSensitive ? "Sensitive notes unlocked" : "Include sensitive notes"}
          </button>

          <button
            type="button"
            disabled={scanning}
            onClick={() => void runScan()}
            className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium hover:bg-slate-800 disabled:opacity-50"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${scanning ? "animate-spin" : ""}`} />
            {scanning ? "Scanning..." : "Scan Workspace"}
          </button>
        </div>
      </div>

      {notice && (
        <div className="flex items-center justify-between gap-2 rounded-lg border border-emerald-800/60 bg-emerald-950/40 px-3 py-2 text-xs text-emerald-200">
          <span>{notice}</span>
          <button
            type="button"
            onClick={() => setNotice(null)}
            className="rounded p-0.5 hover:bg-emerald-900/50"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

      {report && (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
          <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
            <div className="text-[11px] uppercase tracking-wide text-slate-400">
              Relational Integrity
            </div>
            <div className="mt-1 text-xl font-semibold text-emerald-300">
              {report.relational_integrity_score.toFixed(1)}%
            </div>
            <div className="mt-0.5 text-[11px] text-slate-400">
              {report.links_checked} links across {report.notes_scanned} notes
            </div>
          </div>

          <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
            <div className="text-[11px] uppercase tracking-wide text-slate-400">
              Healable Broken Links
            </div>
            <div className="mt-1 text-xl font-semibold text-sky-300">
              {report.healable_links_count} / {report.broken_links_count}
            </div>
            <div className="mt-0.5 text-[11px] text-slate-400">
              {report.move_log_entries} USN/scanner moves tracked
            </div>
          </div>

          <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
            <div className="text-[11px] uppercase tracking-wide text-slate-400">
              Taxonomic Hygiene
            </div>
            <div className="mt-1 text-xl font-semibold text-violet-300">
              {report.taxonomic_hygiene_score.toFixed(1)}%
            </div>
            <div className="mt-0.5 text-[11px] text-slate-400">
              {report.tag_weeds_count} tag weed occurrence(s)
            </div>
          </div>

          <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
            <div className="text-[11px] uppercase tracking-wide text-slate-400">
              Rules &amp; Privacy Gate
            </div>
            <div className="mt-1 text-xl font-semibold text-amber-300">
              {report.rule_path_issues_count} stale rule(s)
            </div>
            <div className="mt-0.5 text-[11px] text-slate-400">
              {report.excluded_sensitive_count} sensitive note(s) hidden until unlocked
            </div>
          </div>
        </div>
      )}

      {/* Filter & Action Toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-slate-800 bg-slate-900/40 p-3">
        <div className="flex flex-wrap items-center gap-2">
          <input
            type="text"
            value={scopeRoot}
            onChange={(e) => setScopeRoot(e.target.value)}
            placeholder="Optional scope folder path (leave blank for all non-backup roots)"
            className="w-72 rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-200 placeholder:text-slate-500"
          />
          <label className="inline-flex items-center gap-1.5 text-xs text-slate-300">
            <input
              type="checkbox"
              checked={includeDismissed}
              onChange={(e) => setIncludeDismissed(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950"
            />
            Show dismissed
          </label>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <button
            type="button"
            disabled={previewing || healableLinkCount + patchableTagClusterCount === 0}
            onClick={() => void handlePreviewDiffs()}
            className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40"
          >
            <Eye className="h-3.5 w-3.5" />
            {previewing ? "Building Diffs..." : `Dry-Run Diff (${healableLinkCount + patchableTagClusterCount})`}
          </button>

          <button
            type="button"
            disabled={staging || healableLinkCount + patchableTagClusterCount === 0}
            onClick={() => void handleStageBatch()}
            className="inline-flex items-center gap-1.5 rounded-lg bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
          >
            <Play className="h-3.5 w-3.5" />
            {staging ? "Staging..." : "Review in Simulator"}
          </button>

          <button
            type="button"
            onClick={onReviewBatches}
            className="inline-flex items-center gap-1.5 rounded-lg border border-emerald-700/60 bg-emerald-950/30 px-2.5 py-1.5 text-xs text-emerald-300 hover:bg-emerald-900/40"
          >
            <Sparkles className="h-3.5 w-3.5" />
            Open Simulator
          </button>
        </div>
      </div>

      {/* Unified Diff Preview Drawer */}
      {diffs && (
        <div className="space-y-3 rounded-xl border border-sky-800/60 bg-slate-900/70 p-4">
          <div className="flex items-center justify-between">
            <h2 className="text-sm font-semibold text-sky-300">
              Dry-Run Unified Diff Preview ({diffs.length} file(s), nothing written yet)
            </h2>
            <button
              type="button"
              onClick={() => setDiffs(null)}
              className="rounded p-1 text-slate-400 hover:bg-slate-800"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          {diffs.length === 0 ? (
            <p className="text-xs text-slate-400">No text modifications computed for current selection.</p>
          ) : (
            <div className="space-y-2.5">
              {diffs.map((f) => (
                <div
                  key={f.file_id}
                  className="rounded-lg border border-slate-800 bg-slate-950/70 p-3"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
                    <span className="font-mono text-slate-200">{f.file_path}</span>
                    <div className="flex items-center gap-2">
                      <span className="text-sky-300">{f.links_healed} link(s)</span>
                      <span className="text-violet-300">{f.tags_pruned} tag(s)</span>
                      {f.blocked_reason && (
                        <span className="rounded bg-rose-950/60 px-2 py-0.5 text-rose-300">
                          {f.blocked_reason}
                        </span>
                      )}
                    </div>
                  </div>
                  <pre className="mt-2 overflow-x-auto rounded bg-slate-900/80 p-2.5 font-mono text-[11px] leading-relaxed text-slate-300">
                    {f.diff_preview}
                  </pre>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {/* Sub-tabs */}
      <div className="flex flex-wrap gap-2 border-b border-slate-800 pb-2 text-xs">
        {[
          {
            id: "links" as const,
            label: `Broken Links (${report?.broken_links.length ?? 0})`,
            icon: Link2,
          },
          {
            id: "tags" as const,
            label: `Tag Weeds (${report?.tag_clusters.length ?? 0})`,
            icon: Tags,
          },
          {
            id: "rules" as const,
            label: `Rule Paths & DB Integrity (${report?.rule_issues.length ?? 0})`,
            icon: FolderCog,
          },
          {
            id: "batches" as const,
            label: `Recent Healing Batches (${batches.length})`,
            icon: History,
          },
        ].map((t) => {
          const Icon = t.icon;
          const active = subTab === t.id;
          return (
            <button
              key={t.id}
              type="button"
              onClick={() => setSubTab(t.id)}
              className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 font-medium ${
                active
                  ? "bg-emerald-600/20 text-emerald-300 border border-emerald-700/60"
                  : "text-slate-400 hover:bg-slate-800/60 hover:text-slate-200"
              }`}
            >
              <Icon className="h-3.5 w-3.5" />
              {t.label}
            </button>
          );
        })}
      </div>

      {/* Broken Links Tab */}
      {subTab === "links" && (
        <div className="space-y-3">
          {!report || report.broken_links.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-6 text-center text-xs text-slate-400">
              All scanned internal Markdown links and wikilinks point cleanly to live files.
            </div>
          ) : (
            report.broken_links.map((item) => {
              const selectable = item.status === "resolved";
              const checked = selectedLinks.has(item.issue_key);
              return (
                <div
                  key={item.issue_key}
                  className="rounded-xl border border-slate-800 bg-slate-900/50 p-4 space-y-2"
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="flex items-start gap-2.5 min-w-0">
                      {selectable && (
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleLinkSelect(item.issue_key)}
                          className="mt-1 rounded border-slate-700 bg-slate-950"
                        />
                      )}
                      <div className="min-w-0">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="font-medium text-sm text-slate-100">
                            {item.source_name}
                          </span>
                          <span className="text-[11px] text-slate-400">
                            line {item.line_number} · {item.drive_label}
                          </span>
                          {statusPill(item.status)}
                          {item.resolution_source && (
                            <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] font-mono text-slate-300">
                              {item.resolution_source}
                            </span>
                          )}
                          {selectable && (
                            <span className="text-[11px] text-emerald-300">
                              {(item.confidence * 100).toFixed(0)}% match
                            </span>
                          )}
                        </div>
                        <div className="truncate font-mono text-[11px] text-slate-400">
                          {item.source_path}
                        </div>
                      </div>
                    </div>

                    <div className="flex items-center gap-1.5 text-xs">
                      <button
                        type="button"
                        disabled={!desktop}
                        onClick={() => void handleInspectFile(item.source_file_id)}
                        className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 hover:bg-slate-800 disabled:opacity-40"
                      >
                        <FileSearch className="h-3 w-3" /> Inspect
                      </button>
                      {item.dismissed ? (
                        <button
                          type="button"
                          disabled={busyId === item.issue_key}
                          onClick={() => void handleUndismiss(item.issue_key)}
                          className="rounded border border-slate-700 px-2 py-1 text-slate-300 hover:bg-slate-800"
                        >
                          Restore
                        </button>
                      ) : (
                        <button
                          type="button"
                          disabled={busyId === item.issue_key}
                          onClick={() => void handleDismiss(item.issue_key, "broken_link")}
                          className="rounded border border-slate-700 px-2 py-1 text-slate-400 hover:bg-slate-800"
                        >
                          Dismiss
                        </button>
                      )}
                    </div>
                  </div>

                  <div className="grid gap-2 sm:grid-cols-2 text-xs">
                    <div className="rounded-lg border border-rose-900/40 bg-rose-950/20 p-2">
                      <div className="text-[10px] uppercase text-rose-300">Broken Target</div>
                      <div className="mt-0.5 break-all font-mono text-rose-200">
                        {item.raw_syntax}
                      </div>
                    </div>
                    <div className="rounded-lg border border-emerald-900/40 bg-emerald-950/20 p-2">
                      <div className="text-[10px] uppercase text-emerald-300">
                        {item.replacement_syntax ? "Healed Target" : "Resolution Guidance"}
                      </div>
                      <div className="mt-0.5 break-all font-mono text-emerald-200">
                        {item.replacement_syntax ?? item.reason}
                      </div>
                    </div>
                  </div>

                  {item.candidate_paths.length > 1 && (
                    <div className="rounded-lg bg-slate-950/60 p-2 text-[11px] text-slate-300">
                      <span className="font-medium text-orange-300">Candidates found: </span>
                      {item.candidate_paths.join(" · ")}
                    </div>
                  )}
                </div>
              );
            })
          )}
        </div>
      )}

      {/* Tag Weeds Tab */}
      {subTab === "tags" && (
        <div className="space-y-3">
          {!report || report.tag_clusters.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-6 text-center text-xs text-slate-400">
              No casing discrepancies, near-duplicate typos, or hex color false positives found.
            </div>
          ) : (
            report.tag_clusters.map((cluster: TagWeedCluster) => {
              const selectable = cluster.patchable_count > 0;
              const checked = selectedTags.has(cluster.issue_key);
              return (
                <div
                  key={cluster.issue_key}
                  className="rounded-xl border border-slate-800 bg-slate-900/50 p-4 space-y-2.5"
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="flex items-start gap-2.5">
                      {selectable && (
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleTagSelect(cluster.issue_key)}
                          className="mt-1 rounded border-slate-700 bg-slate-950"
                        />
                      )}
                      <div>
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="inline-flex items-center gap-1 font-semibold text-sm text-slate-100">
                            <Hash className="h-3.5 w-3.5 text-violet-400" />
                            {cluster.kind === "hex_color_false_positive"
                              ? `${cluster.canonical_tag} (Hex Color False Positive)`
                              : `Consolidate -> ${cluster.canonical_tag}`}
                          </span>
                          <span className="rounded bg-violet-950/60 px-2 py-0.5 text-[11px] text-violet-300 border border-violet-800/50">
                            {cluster.kind}
                          </span>
                          <span className="text-[11px] text-slate-400">
                            {cluster.patchable_count}/{cluster.total_occurrences} auto-patchable
                          </span>
                        </div>
                        <p className="mt-0.5 text-xs text-slate-300">{cluster.reason}</p>
                      </div>
                    </div>

                    <button
                      type="button"
                      disabled={busyId === cluster.issue_key}
                      onClick={() =>
                        void (cluster.dismissed
                          ? handleUndismiss(cluster.issue_key)
                          : handleDismiss(cluster.issue_key, "tag_weed"))
                      }
                      className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300 hover:bg-slate-800"
                    >
                      {cluster.dismissed ? "Restore" : "Dismiss"}
                    </button>
                  </div>

                  <div className="space-y-1.5">
                    {cluster.occurrences.map((occ) => (
                      <div
                        key={occ.issue_key}
                        className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-950/60 px-3 py-2 text-xs"
                      >
                        <div className="min-w-0">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="font-mono text-slate-200">{occ.file_name}</span>
                            <span className="text-slate-400">L{occ.line_number}</span>
                            <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-300">
                              {occ.in_frontmatter ? "YAML frontmatter" : "Markdown body"}
                            </span>
                            <span className="font-mono text-amber-300">{occ.raw_tag}</span>
                            {occ.canonical_tag && (
                              <span className="font-mono text-emerald-300">
                                -&gt; {occ.canonical_tag}
                              </span>
                            )}
                            {!occ.patchable && (
                              <span className="text-[11px] text-slate-400">
                                (advisory only to preserve prose)
                              </span>
                            )}
                          </div>
                          <div className="truncate font-mono text-[11px] text-slate-400">
                            {occ.snippet}
                          </div>
                        </div>
                        <button
                          type="button"
                          disabled={!desktop}
                          onClick={() => void handleInspectFile(occ.file_id)}
                          className="rounded border border-slate-700 px-2 py-1 text-[11px] hover:bg-slate-800 disabled:opacity-40"
                        >
                          Inspect
                        </button>
                      </div>
                    ))}
                  </div>
                </div>
              );
            })
          )}
        </div>
      )}

      {/* Rule Paths & DB Integrity Tab */}
      {subTab === "rules" && (
        <div className="space-y-4">
          <div className="space-y-3">
            <h2 className="text-sm font-semibold">Folderless Rule Path Audits</h2>
            {!report || report.rule_issues.length === 0 ? (
              <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-4 text-xs text-slate-400">
                All enabled Folderless Rule scope &amp; destination paths are valid.
              </div>
            ) : (
              report.rule_issues.map((issue) => (
                <div
                  key={issue.issue_key}
                  className="rounded-xl border border-slate-800 bg-slate-900/50 p-4 space-y-2.5"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <div>
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-sm text-slate-100">
                          {issue.rule_name}
                        </span>
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-[10px] text-slate-300">
                          {issue.field}
                        </span>
                        {statusPill(issue.status)}
                      </div>
                      <p className="mt-0.5 text-xs text-slate-400">{issue.reason}</p>
                    </div>
                  </div>

                  <div className="grid gap-2 sm:grid-cols-2 text-xs">
                    <div>
                      <div className="mb-1 text-[10px] uppercase text-slate-400">Stale Path</div>
                      <div className="rounded bg-slate-950 px-2.5 py-1.5 font-mono text-rose-300 break-all">
                        {issue.current_path}
                      </div>
                    </div>
                    <div>
                      <div className="mb-1 text-[10px] uppercase text-slate-400">
                        Updated Path
                      </div>
                      <div className="flex gap-2">
                        <input
                          type="text"
                          value={ruleEdits[issue.issue_key] ?? issue.suggested_path ?? ""}
                          onChange={(e) =>
                            setRuleEdits((prev) => ({
                              ...prev,
                              [issue.issue_key]: e.target.value,
                            }))
                          }
                          placeholder="Enter corrected folder path..."
                          className="flex-1 rounded border border-slate-700 bg-slate-950 px-2.5 py-1 font-mono text-xs text-slate-200"
                        />
                        <button
                          type="button"
                          disabled={busyId === issue.issue_key}
                          onClick={() => void handleApplyRuleFix(issue)}
                          className="rounded bg-emerald-600 px-3 py-1 font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                        >
                          Update Rule
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>

          {/* Transaction / Journal Verification Card */}
          <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4 space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <Stethoscope className="h-4 w-4 text-emerald-400" />
                <div>
                  <h2 className="text-sm font-semibold">
                    Encrypted Transaction &amp; FTS5 Journal Verification (`D1`)
                  </h2>
                  <p className="text-xs text-slate-400">
                    Verifies SQLCipher `quick_check`, foreign keys, FTS5 sync, and outbox health after
                    self-healing writes.
                  </p>
                </div>
              </div>
              <div className="flex gap-2 text-xs">
                <button
                  type="button"
                  disabled={busyId === "doctor"}
                  onClick={() => void handleDoctorVerify(false)}
                  className="rounded border border-slate-700 px-2.5 py-1 hover:bg-slate-800"
                >
                  Verify DB Journal
                </button>
                <button
                  type="button"
                  disabled={busyId === "doctor"}
                  onClick={() => void handleDoctorVerify(true)}
                  className="rounded bg-slate-800 px-2.5 py-1 hover:bg-slate-700"
                >
                  Repair Sync Drift
                </button>
              </div>
            </div>

            {doctor && (
              <div className="grid gap-2 sm:grid-cols-2">
                {doctor.checks.slice(0, 4).map((c) => (
                  <div
                    key={c.check_id}
                    className="rounded-lg border border-slate-800 bg-slate-950/60 p-2.5 text-xs"
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-medium text-slate-200">{c.name}</span>
                      <span className={c.passed ? "text-emerald-300" : "text-rose-300"}>
                        {c.passed ? "PASS" : "FAIL"}
                      </span>
                    </div>
                    <div className="mt-1 font-mono text-[11px] text-slate-400">{c.details}</div>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Recent Healing Batches Tab */}
      {subTab === "batches" && (
        <div className="space-y-2.5">
          {batches.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-6 text-center text-xs text-slate-400">
              No self-healing batches staged or executed yet.
            </div>
          ) : (
            batches.map((b) => (
              <div
                key={b.batch_id}
                className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-slate-800 bg-slate-900/50 p-3 text-xs"
              >
                <div>
                  <div className="flex items-center gap-2">
                    <span className="font-semibold text-slate-100">{b.summary_label}</span>
                    <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-[10px] text-slate-300">
                      {b.status}
                    </span>
                  </div>
                  <div className="mt-0.5 text-slate-400">
                    Batch {b.batch_id.slice(0, 8)} · {formatDateTime(b.created_at)} ·{" "}
                    {b.executed_count} executed / {b.rolled_back_count} rolled back
                  </div>
                </div>
                {b.can_undo && (
                  <button
                    type="button"
                    disabled={busyId === b.batch_id}
                    onClick={() => void handleRollbackBatch(b.batch_id)}
                    className="inline-flex items-center gap-1 rounded bg-amber-700 px-2.5 py-1 font-medium text-white hover:bg-amber-600 disabled:opacity-40"
                  >
                    <RotateCcw className="h-3 w-3" /> Undo Batch
                  </button>
                )}
              </div>
            ))
          )}
        </div>
      )}

      {/* File Inspection Modal */}
      {inspected && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4">
          <div className="max-h-[85vh] w-full max-w-2xl overflow-y-auto rounded-2xl border border-slate-700 bg-slate-900 p-5 space-y-3 text-xs">
            <div className="flex items-center justify-between">
              <h3 className="text-sm font-semibold text-slate-100">{inspected.name}</h3>
              <button
                type="button"
                onClick={() => setInspected(null)}
                className="rounded p-1 hover:bg-slate-800"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="font-mono text-[11px] text-slate-400 break-all">
              {inspected.path} · {formatBytes(inspected.size)}
            </div>
            <pre className="whitespace-pre-wrap rounded-lg bg-slate-950 p-3 font-mono text-[11px] text-slate-300">
              {inspected.chunks_preview.length > 0
                ? inspected.chunks_preview.join("\n---\n")
                : "(No indexed chunks preview)"}
            </pre>
          </div>
        </div>
      )}
    </div>
  );
}
