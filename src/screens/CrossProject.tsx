import { useCallback, useEffect, useState } from "react";
import { Flash } from "../ui";
import {
  AlertTriangle,
  ArrowRight,
  Check,
  CheckCircle2,
  Copy,
  EyeOff,
  FileSearch,
  FolderGit2,
  GitCompare,
  Network,
  RefreshCw,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type CrossProjectBrief,
  type CrossProjectEdge,
  type CrossProjectFileRef,
  type CrossProjectFinding,
  type CrossProjectReport,
  type DependencyKind,
  type FileDetail,
  type FindingKind,
  type InsightStatus,
} from "../bindings/client";
import { formatBytes } from "../format";
import {
  PREVIEW_CROSS_PROJECT_BRIEF,
  PREVIEW_CROSS_PROJECT_REPORT,
} from "./fixtures/crossProject";

type SubTab = "findings" | "dependencies" | "projects";

function formatShortDate(ms: number): string {
  if (!ms) return "Unknown";
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

function findingKindLabel(k: FindingKind): string {
  switch (k) {
    case "duplicate_effort":
      return "Duplicate Effort";
    case "conflicting_decision":
      return "Decision Conflict";
    case "version_divergence":
      return "Version Drift";
  }
}

function dependencyKindLabel(k: DependencyKind): string {
  switch (k) {
    case "shared_stakeholder":
      return "Shared Stakeholder";
    case "shared_decision":
      return "Shared Decision";
    case "cross_project_action":
      return "Cross-Project Action";
    case "document_reference":
      return "Document Mention";
    case "shared_stack":
      return "Shared Stack";
  }
}

export function CrossProject({ desktop }: { desktop: boolean }) {
  const [report, setReport] = useState<CrossProjectReport | null>(
    desktop ? null : PREVIEW_CROSS_PROJECT_REPORT
  );
  const [brief, setBrief] = useState<CrossProjectBrief | null>(null);
  const [subTab, setSubTab] = useState<SubTab>("findings");

  const [projectFilter, setProjectFilter] = useState<string>("");
  const [kindFilter, setKindFilter] = useState<FindingKind | "">("");
  const [includeDesignSets, setIncludeDesignSets] = useState(true);
  const [includeDismissed, setIncludeDismissed] = useState(false);
  const [includeSensitive, setIncludeSensitive] = useState(false);

  const [loading, setLoading] = useState(false);
  const [synthesizing, setSynthesizing] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const runScan = useCallback(async () => {
    if (!desktop) {
      const pid = projectFilter || null;
      const filteredFindings = PREVIEW_CROSS_PROJECT_REPORT.findings.filter((f) => {
        if (!includeDismissed && f.status === "dismissed") return false;
        if (pid && f.project_a_id !== pid && f.project_b_id !== pid) return false;
        if (kindFilter && f.kind !== kindFilter) return false;
        return true;
      });
      const filteredEdges = PREVIEW_CROSS_PROJECT_REPORT.edges.filter(
        (e) => !pid || e.from_project_id === pid || e.to_project_id === pid
      );
      setReport({
        ...PREVIEW_CROSS_PROJECT_REPORT,
        findings: filteredFindings,
        edges: filteredEdges,
      });
      return;
    }

    setLoading(true);
    setNotice(null);
    try {
      const next = await aegis.crossProject.scan({
        project_id: projectFilter || null,
        kind: kindFilter || null,
        include_design_sets: includeDesignSets,
        include_dismissed: includeDismissed,
        include_sensitive: includeSensitive,
      });
      setReport(next);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [desktop, projectFilter, kindFilter, includeDesignSets, includeDismissed, includeSensitive]);

  useEffect(() => {
    void runScan();
  }, [runScan]);

  const handleSynthesizeBrief = async () => {
    if (!desktop) {
      setBrief(PREVIEW_CROSS_PROJECT_BRIEF);
      return;
    }
    setSynthesizing(true);
    setNotice(null);
    try {
      const res = await aegis.crossProject.synthesizeBrief({
        projectAId: projectFilter || null,
        projectBId: null,
        includeSensitive,
      });
      setBrief(res);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setSynthesizing(false);
    }
  };

  const handleTriage = async (
    finding: CrossProjectFinding,
    status: InsightStatus
  ) => {
    if (!desktop) {
      setNotice(`Preview mode: finding status (${status}) is saved in the desktop app.`);
      return;
    }
    setBusyKey(finding.insight_key);
    setNotice(null);
    try {
      await aegis.crossProject.setInsightStatus(finding.insight_key, status);
      await runScan();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyKey(null);
    }
  };

  const handleInspect = async (fileId: string) => {
    if (!desktop) {
      setNotice("File detail inspection is available inside the Tauri desktop runtime.");
      return;
    }
    try {
      setInspected(await aegis.file.get(fileId));
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const handleCopyPath = async (fileRef: CrossProjectFileRef) => {
    try {
      await navigator.clipboard.writeText(fileRef.path);
      setCopiedId(fileRef.file_id);
      setTimeout(() => setCopiedId((p) => (p === fileRef.file_id ? null : p)), 1500);
    } catch {
      setNotice(fileRef.path);
    }
  };

  const renderFileCard = (label: string, rf: CrossProjectFileRef | null) => {
    if (!rf) return null;
    return (
      <div className="rounded-lg border border-slate-800 bg-slate-900/70 p-2.5 text-xs">
        <div className="flex items-center justify-between gap-2">
          <div className="min-w-0">
            <span className="text-xs uppercase tracking-wide text-slate-400">
              {label} · {rf.collection_name}
            </span>
            <div className="truncate font-semibold text-slate-100">{rf.filename}</div>
          </div>
          <div className="flex shrink-0 items-center gap-1">
            <button
              type="button"
              onClick={() => void handleInspect(rf.file_id)}
              className="rounded border border-slate-700 px-1.5 py-0.5 text-xs text-slate-200 hover:bg-slate-800 transition-colors"
            >
              <FileSearch className="mr-0.5 inline h-2.5 w-2.5" />
              Inspect
            </button>
            <button
              type="button"
              onClick={() => void handleCopyPath(rf)}
              className="rounded border border-slate-700 px-1.5 py-0.5 text-xs text-slate-200 hover:bg-slate-800 transition-colors"
            >
              {copiedId === rf.file_id ? (
                <Check className="h-2.5 w-2.5 text-emerald-400" />
              ) : (
                <Copy className="h-2.5 w-2.5" />
              )}
            </button>
          </div>
        </div>
        <div className="mt-1 truncate font-mono text-xs text-slate-400">{rf.path}</div>
        {rf.excerpt && (
          <p className="mt-1.5 line-clamp-2 border-l-2 border-slate-700 pl-2 text-xs text-slate-300">
            {rf.excerpt}
          </p>
        )}
      </div>
    );
  };

  return (
    <div className="space-y-5">
      {/* Header */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h1 className="flex flex-wrap items-center gap-2.5 text-lg font-semibold text-slate-50">
              <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-slate-800 bg-slate-900 text-emerald-400"><Network className="h-4 w-4" aria-hidden="true" /></span>
              Cross-Project
              <span className="inline-flex items-center gap-1 rounded-full border border-emerald-700/60 bg-emerald-950/50 px-2 py-0.5 text-xs font-medium text-emerald-200">
                <ShieldCheck className="h-3 w-3" aria-hidden="true" />
                Reads files on disk only
              </span>
            </h1>
            <p className="mt-1.5 max-w-3xl text-sm leading-relaxed text-slate-400">
              Spots work you are repeating in several projects, decisions that contradict each other, libraries drifting apart in version, and projects that depend on each other.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={loading}
              onClick={() => void runScan()}
              className="flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
            >
              <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
              Rescan Projects
            </button>
            <button
              type="button"
              disabled={
                !report ||
                (report.findings.length === 0 && report.edges.length === 0) ||
                synthesizing
              }
              onClick={() => void handleSynthesizeBrief()}
              className="flex items-center gap-1.5 rounded-md border border-violet-600/60 bg-violet-950/40 px-3 py-1.5 text-xs font-semibold text-violet-200 hover:bg-violet-900/50 disabled:opacity-40"
            >
              <Sparkles className="h-3.5 w-3.5" />
              {synthesizing ? "Writing brief…" : "Write brief (local model)"}
            </button>
          </div>
        </div>
      </section>

      <Flash message={notice} tone="warning" onDismiss={() => setNotice(null)} />

      {/* KPI Strip */}
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs text-slate-400">Mapped Projects</div>
          <div className="mt-1 text-xl font-bold text-slate-100">
            {report?.total_projects ?? 0}
          </div>
          <div className="text-xs text-slate-400">
            {report?.used_vectors ? "Vectors + Manifests + Graph" : "Lexical + Manifests + Graph"}
          </div>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs text-slate-400">Duplicate Effort</div>
          <div className="mt-1 text-xl font-bold text-amber-300">
            {report?.duplicate_effort_count ?? 0}
          </div>
          <div className="text-xs text-slate-400">Cross-project overlap alerts</div>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs text-slate-400">Conflicts &amp; Drift</div>
          <div className="mt-1 text-xl font-bold text-rose-300">
            {report?.conflict_count ?? 0}
          </div>
          <div className="text-xs text-slate-400">Decisions &amp; major versions</div>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs text-slate-400">Dependency Links</div>
          <div className="mt-1 text-xl font-bold text-emerald-300">
            {report?.dependency_edge_count ?? 0}
          </div>
          <div className="text-xs text-slate-400">
            {report ? `Scanned in ${report.took_ms} ms` : "Awaiting scan"}
          </div>
        </div>
      </div>

      {/* Filters & Sub-tabs */}
      <div className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-slate-800 bg-slate-900/60 p-3">
        <div className="inline-flex rounded-lg border border-slate-800 bg-slate-950 p-0.5 text-xs">
          <button
            type="button"
            onClick={() => setSubTab("findings")}
            className={`rounded-md px-3 py-1 font-medium transition ${
              subTab === "findings"
                ? "bg-emerald-500/20 text-emerald-200"
                : "text-slate-400 hover:text-slate-200"
            }`}
          >
            Overlaps &amp; Conflicts ({report?.findings.length ?? 0})
          </button>
          <button
            type="button"
            onClick={() => setSubTab("dependencies")}
            className={`rounded-md px-3 py-1 font-medium transition ${
              subTab === "dependencies"
                ? "bg-emerald-500/20 text-emerald-200"
                : "text-slate-400 hover:text-slate-200"
            }`}
          >
            Dependency Map ({report?.edges.length ?? 0})
          </button>
          <button
            type="button"
            onClick={() => setSubTab("projects")}
            className={`rounded-md px-3 py-1 font-medium transition ${
              subTab === "projects"
                ? "bg-emerald-500/20 text-emerald-200"
                : "text-slate-400 hover:text-slate-200"
            }`}
          >
            Projects ({report?.projects.length ?? 0})
          </button>
        </div>

        <div className="flex flex-wrap items-center gap-2.5 text-xs text-slate-300">
          <select
            aria-label="Filter by project"
            value={projectFilter}
            onChange={(e) => setProjectFilter(e.target.value)}
            className="rounded-md border border-slate-700 bg-slate-950 px-2.5 py-1 text-xs text-slate-200"
          >
            <option value="">All projects</option>
            {report?.projects.map((p) => (
              <option key={p.collection_id} value={p.collection_id}>
                {p.name}
              </option>
            ))}
          </select>

          <select
            aria-label="Filter by finding kind"
            value={kindFilter}
            onChange={(e) => setKindFilter(e.target.value as FindingKind | "")}
            className="rounded-md border border-slate-700 bg-slate-950 px-2.5 py-1 text-xs text-slate-200"
          >
            <option value="">All findings</option>
            <option value="duplicate_effort">Duplicate effort</option>
            <option value="conflicting_decision">Conflicting decisions</option>
            <option value="version_divergence">Version divergence</option>
          </select>

          <label className="inline-flex cursor-pointer items-center gap-1">
            <input
              type="checkbox"
              checked={includeDesignSets}
              onChange={(e) => setIncludeDesignSets(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950 text-emerald-500"
            />
            Design sets
          </label>
          <label className="inline-flex cursor-pointer items-center gap-1">
            <input
              type="checkbox"
              checked={includeDismissed}
              onChange={(e) => setIncludeDismissed(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950 text-slate-500"
            />
            Dismissed ({report?.dismissed_count ?? 0})
          </label>
          <label className="inline-flex cursor-pointer items-center gap-1">
            <input
              type="checkbox"
              checked={includeSensitive}
              onChange={(e) => setIncludeSensitive(e.target.checked)}
              className="rounded border-slate-700 bg-slate-950 text-amber-500"
            />
            Sensitive
          </label>
        </div>
      </div>

      {report && report.excluded_sensitive_count > 0 && (
        <div className="flex items-center gap-2 rounded-lg border border-amber-800/50 bg-amber-950/30 px-3 py-2 text-xs text-amber-200">
          <ShieldAlert className="h-4 w-4 shrink-0 text-amber-400" />
          <span>
            {report.excluded_sensitive_count} sensitive project file(s) left out until you unlock sensitive files with Windows.
          </span>
        </div>
      )}

      {/* Grounded Local LLM Synthesis Brief */}
      {brief && (
        <section className="rounded-xl border border-violet-700/50 bg-violet-950/20 p-4">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <h3 className="flex items-center gap-1.5 text-xs font-semibold text-violet-200">
              <Sparkles className="h-3.5 w-3.5 text-violet-400" />
              {brief.title}
            </h3>
            <span className="text-xs text-slate-400">
              {brief.model_name} ({brief.backend}) · {brief.duration_ms} ms
            </span>
          </div>
          <p className="mt-2 whitespace-pre-wrap text-xs leading-relaxed text-slate-200">
            {brief.summary}
          </p>
          <div className="mt-3 flex flex-wrap gap-2">
            {brief.citations.map((c) => (
              <button
                key={c.file_id}
                type="button"
                onClick={() => void handleInspect(c.file_id)}
                className="rounded border border-slate-700 bg-slate-950/80 px-2 py-1 text-xs text-slate-200 hover:border-violet-500"
              >
                [{c.ref_id}] {c.filename} ({c.project_name})
              </button>
            ))}
          </div>
        </section>
      )}

      {/* Sub-Tab Content */}
      {subTab === "findings" && (
        <div className="space-y-3">
          {report && report.findings.length > 0 ? (
            report.findings.map((f) => {
              const isBusy = busyKey === f.insight_key;
              return (
                <article
                  key={f.insight_key}
                  className={`rounded-xl border p-4 transition ${
                    f.status === "dismissed"
                      ? "border-slate-800/60 bg-slate-950/40 opacity-65"
                      : f.severity === "high"
                        ? "border-rose-800/50 bg-slate-900/60"
                        : "border-slate-800 bg-slate-900/60"
                  }`}
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="space-y-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span
                          className={`inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs font-semibold ${
                            f.kind === "conflicting_decision"
                              ? "bg-rose-500/20 text-rose-300"
                              : f.kind === "version_divergence"
                                ? "bg-indigo-500/20 text-indigo-300"
                                : "bg-amber-500/20 text-amber-300"
                          }`}
                        >
                          {f.kind === "conflicting_decision" ? (
                            <AlertTriangle className="h-3 w-3" />
                          ) : (
                            <GitCompare className="h-3 w-3" />
                          )}
                          {findingKindLabel(f.kind)}
                        </span>
                        <span className="rounded bg-slate-800 px-2 py-0.5 text-xs font-medium text-emerald-300">
                          {f.project_a_name} ↔ {f.project_b_name}
                        </span>
                        <span className="rounded bg-slate-800 px-2 py-0.5 text-xs uppercase text-slate-300">
                          {f.severity} · {f.status}
                        </span>
                      </div>
                      <h3 className="text-sm font-semibold text-slate-100">{f.title}</h3>
                      <p className="text-xs text-slate-300">{f.summary}</p>
                    </div>

                    <div className="flex flex-wrap items-center gap-1.5">
                      {f.status !== "acknowledged" && f.status !== "dismissed" && (
                        <button
                          type="button"
                          disabled={isBusy}
                          onClick={() => void handleTriage(f, "acknowledged")}
                          className="rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                        >
                          Acknowledge
                        </button>
                      )}
                      {f.status !== "resolved" && (
                        <button
                          type="button"
                          disabled={isBusy}
                          onClick={() => void handleTriage(f, "resolved")}
                          className="inline-flex items-center gap-1 rounded border border-emerald-700/60 bg-emerald-950/40 px-2.5 py-1 text-xs text-emerald-200 hover:bg-emerald-900/50 disabled:opacity-40"
                        >
                          <CheckCircle2 className="h-3 w-3" />
                          Resolve
                        </button>
                      )}
                      {f.status === "dismissed" || f.status === "resolved" || f.status === "acknowledged" ? (
                        <button
                          type="button"
                          disabled={isBusy}
                          onClick={() => void handleTriage(f, "open")}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                        >
                          <RotateCcw className="h-3 w-3" />
                          Reopen
                        </button>
                      ) : null}
                      {f.status !== "dismissed" && (
                        <button
                          type="button"
                          disabled={isBusy}
                          onClick={() => void handleTriage(f, "dismissed")}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-400 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                        >
                          <EyeOff className="h-3 w-3" />
                          Dismiss
                        </button>
                      )}
                    </div>
                  </div>

                  {f.note && (
                    <p className="mt-2 rounded bg-slate-950/70 px-2.5 py-1 text-xs text-amber-200">
                      Note: {f.note}
                    </p>
                  )}

                  {(f.file_a || f.file_b) && (
                    <div className="mt-3 grid grid-cols-1 gap-2.5 md:grid-cols-2">
                      {renderFileCard("Project A", f.file_a)}
                      {renderFileCard("Project B", f.file_b)}
                    </div>
                  )}

                  {f.shared_terms.length > 0 && (
                    <div className="mt-2.5 flex flex-wrap gap-1.5 text-xs">
                      {f.shared_terms.map((t) => (
                        <span
                          key={t}
                          className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300"
                        >
                          #{t}
                        </span>
                      ))}
                    </div>
                  )}
                </article>
              );
            })
          ) : (
            <div className="rounded-xl border border-dashed border-slate-800 p-8 text-center text-xs text-slate-400">
              No duplicate effort, decision conflicts, or version drift detected across the
              selected projects.
            </div>
          )}
        </div>
      )}

      {subTab === "dependencies" && (
        <div className="space-y-3">
          {report && report.edges.length > 0 ? (
            report.edges.map((e: CrossProjectEdge) => (
              <article
                key={e.edge_key}
                className="rounded-xl border border-slate-800 bg-slate-900/60 p-4"
              >
                <div className="flex flex-wrap items-center justify-between gap-2">
                  <div className="flex flex-wrap items-center gap-2">
                    <span className="rounded bg-sky-500/15 px-2 py-0.5 text-xs font-semibold text-sky-300">
                      {dependencyKindLabel(e.kind)}
                    </span>
                    <span className="inline-flex items-center gap-1.5 text-sm font-semibold text-slate-100">
                      {e.from_project_name}
                      {e.directed ? (
                        <ArrowRight className="h-3.5 w-3.5 text-emerald-400" />
                      ) : (
                        <span className="text-slate-400">↔</span>
                      )}
                      {e.to_project_name}
                    </span>
                  </div>
                  <span className="rounded bg-slate-800 px-2 py-0.5 text-xs font-medium text-emerald-300">
                    Strength {Math.round(e.strength * 100)}%
                  </span>
                </div>
                <p className="mt-1.5 text-xs text-slate-300">{e.summary}</p>
                {e.shared_labels.length > 0 && (
                  <div className="mt-2.5 flex flex-wrap gap-1.5 text-xs">
                    {e.shared_labels.map((lbl) => (
                      <span
                        key={lbl}
                        className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-slate-200"
                      >
                        {lbl}
                      </span>
                    ))}
                  </div>
                )}
              </article>
            ))
          ) : (
            <div className="rounded-xl border border-dashed border-slate-800 p-8 text-center text-xs text-slate-400">
              No cross-project dependencies mapped yet. Run Knowledge Extraction or scan project
              manifests to connect shared stakeholders, decisions, actions, and stacks.
            </div>
          )}
        </div>
      )}

      {subTab === "projects" && (
        <div className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-3">
          {report && report.projects.length > 0 ? (
            report.projects.map((p) => (
              <article
                key={p.collection_id}
                className="flex flex-col justify-between rounded-xl border border-slate-800 bg-slate-900/60 p-4"
              >
                <div>
                  <div className="flex items-start justify-between gap-2">
                    <div>
                      <h3 className="flex items-center gap-1.5 text-sm font-semibold text-slate-100">
                        <FolderGit2 className="h-4 w-4 text-emerald-400" />
                        {p.name}
                      </h3>
                      <p className="mt-0.5 text-xs text-slate-400">
                        {p.project_kind ?? p.kind} · {p.file_count} files ·{" "}
                        {formatBytes(p.total_bytes)}
                      </p>
                    </div>
                    {p.finding_count > 0 && (
                      <span className="rounded bg-amber-500/20 px-2 py-0.5 text-xs font-semibold text-amber-300">
                        {p.finding_count} alerts
                      </span>
                    )}
                  </div>
                  {p.root_paths[0] && (
                    <p className="mt-1.5 truncate font-mono text-xs text-slate-500">
                      {p.root_paths[0]}
                    </p>
                  )}
                  <div className="mt-3 grid grid-cols-3 gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-2 text-center text-xs">
                    <div>
                      <div className="font-semibold text-slate-100">{p.related_project_count}</div>
                      <div className="text-xs text-slate-400">Links</div>
                    </div>
                    <div>
                      <div className="font-semibold text-slate-100">{p.decision_count}</div>
                      <div className="text-xs text-slate-400">Decisions</div>
                    </div>
                    <div>
                      <div className="font-semibold text-slate-100">{p.open_action_count}</div>
                      <div className="text-xs text-slate-400">Open Actions</div>
                    </div>
                  </div>
                  {p.top_terms.length > 0 && (
                    <div className="mt-2.5 flex flex-wrap gap-1 text-xs">
                      {p.top_terms.slice(0, 6).map((t) => (
                        <span
                          key={t}
                          className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300"
                        >
                          {t}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
                <div className="mt-3 flex items-center justify-between border-t border-slate-800 pt-2.5 text-xs text-slate-400">
                  <span>Active {formatShortDate(p.last_activity_at)}</span>
                  <button
                    type="button"
                    onClick={() => {
                      setProjectFilter(p.collection_id);
                      setSubTab("findings");
                    }}
                    className="font-medium text-emerald-300 hover:underline"
                  >
                    Focus project →
                  </button>
                </div>
              </article>
            ))
          ) : (
            <div className="col-span-full rounded-xl border border-dashed border-slate-800 p-8 text-center text-xs text-slate-400">
              No active recognised projects found yet. Run project recognition from Filing to
              populate cross-project maps.
            </div>
          )}
        </div>
      )}

      {/* Read-only File Inspection Modal */}
      {inspected && (
        <div
          role="dialog"
          aria-modal="true"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
        >
          <div className="w-full max-w-lg rounded-2xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
            <div className="flex items-center justify-between border-b border-slate-800 pb-3">
              <h4 className="text-sm font-semibold text-slate-100">{inspected.name}</h4>
              <button
                type="button"
                onClick={() => setInspected(null)}
                className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-100"
                aria-label="Close inspection dialog"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="mt-3 space-y-2 text-xs text-slate-300">
              <div className="font-mono text-xs text-slate-400">{inspected.path}</div>
              <div>
                State: <strong>{inspected.state}</strong> · Sensitivity:{" "}
                <strong>{inspected.sensitivity}</strong> · Size:{" "}
                <strong>{formatBytes(inspected.size)}</strong>
              </div>
              {inspected.chunks_preview.length > 0 && (
                <div className="max-h-64 space-y-2 overflow-y-auto rounded-lg border border-slate-800 bg-slate-950 p-3 text-slate-200">
                  {inspected.chunks_preview.map((chunk, idx) => (
                    <p key={idx} className="whitespace-pre-wrap leading-relaxed">
                      {chunk}
                    </p>
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
