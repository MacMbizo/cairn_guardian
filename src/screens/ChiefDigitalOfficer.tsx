import { useCallback, useEffect, useState } from "react";
import {
  Activity,
  AlertTriangle,
  ArrowUpRight,
  CheckCircle2,
  Clock,
  EyeOff,
  FileText,
  Gauge,
  History,
  Lock,
  RefreshCw,
  RotateCcw,
  ShieldCheck,
  Sparkles,
  Unlock,
  XCircle,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type CdoDashboard,
  type CdoExecutiveBriefing,
  type CdoRecommendation,
  type CdoSeverity,
  type Screen,
} from "../bindings/client";
import { PREVIEW_CDO_DASHBOARD } from "./fixtures/cdo";

type CdoSubTab = "recommendations" | "bottlenecks" | "briefings";

function formatDateTime(ms: number | null): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function severityBadge(sev: CdoSeverity) {
  switch (sev) {
    case "critical":
      return (
        <span className="rounded border border-rose-800/60 bg-rose-950/60 px-2 py-0.5 text-xs font-medium text-rose-300">
          Critical
        </span>
      );
    case "high":
      return (
        <span className="rounded border border-amber-800/60 bg-amber-950/60 px-2 py-0.5 text-xs font-medium text-amber-300">
          High
        </span>
      );
    case "medium":
      return (
        <span className="rounded border border-sky-800/60 bg-sky-950/60 px-2 py-0.5 text-xs font-medium text-sky-300">
          Medium
        </span>
      );
    case "low":
      return (
        <span className="rounded border border-slate-700 bg-slate-800/80 px-2 py-0.5 text-xs font-medium text-slate-300">
          Low
        </span>
      );
  }
}

function pillarStyle(status: string): string {
  switch (status) {
    case "healthy":
      return "border-emerald-900/50 bg-emerald-950/20 text-emerald-300";
    case "attention":
      return "border-amber-900/50 bg-amber-950/20 text-amber-300";
    default:
      return "border-rose-900/50 bg-rose-950/20 text-rose-300";
  }
}

function renderProposalPreview(rec: CdoRecommendation) {
  const p = rec.proposal;
  switch (p.type) {
    case "create_folderless_rule":
      return (
        <div className="mt-2 rounded-lg border border-slate-800 bg-slate-950/70 p-2.5 text-xs text-slate-300">
          <div className="font-medium text-indigo-300">
            Draft filing rule: {p.draft.name}
          </div>
          <div className="mt-1 text-slate-400">
            Destination: <code className="text-slate-200">{p.draft.dest_root}</code> · Template:{" "}
            <code className="text-slate-200">{p.draft.path_template}</code>
          </div>
        </div>
      );
    case "create_privacy_rule":
      return (
        <div className="mt-2 rounded-lg border border-slate-800 bg-slate-950/70 p-2.5 text-xs text-slate-300">
          <div className="font-medium text-amber-300">
            Draft privacy rule: {p.draft.name}
          </div>
          <div className="mt-1 text-slate-400">
            Scope: <code className="text-slate-200">{p.draft.path_prefix ?? "Any"}</code> ·
            Pattern: <code className="text-slate-200">{p.draft.filename_pattern ?? "*"}</code> ·
            Action: <code className="text-slate-200">{p.draft.rule_action}</code>
          </div>
        </div>
      );
    case "adjust_setting":
      return (
        <div className="mt-2 rounded-lg border border-slate-800 bg-slate-950/70 p-2.5 text-xs text-slate-300">
          <div className="font-medium text-sky-300">Setting Adjustment: {p.display_label}</div>
          <div className="mt-1 text-slate-400">
            Key: <code className="text-slate-200">{p.key}</code> ({p.scope}) ={" "}
            <code className="text-slate-200">{p.value_json}</code>
          </div>
        </div>
      );
    case "navigate_to_screen":
      return null;
  }
}

export function ChiefDigitalOfficer({
  desktop,
  onNavigate,
}: {
  desktop: boolean;
  onNavigate: (screen: Screen) => void;
}) {
  const [subTab, setSubTab] = useState<CdoSubTab>("recommendations");
  const [horizonDays, setHorizonDays] = useState<number>(30);
  const [includeSensitive, setIncludeSensitive] = useState(false);
  const [includeDismissed, setIncludeDismissed] = useState(false);
  const [sensitiveUnlocked, setSensitiveUnlocked] = useState(false);

  const [dashboard, setDashboard] = useState<CdoDashboard>(PREVIEW_CDO_DASHBOARD);
  const [briefings, setBriefings] = useState<CdoExecutiveBriefing[]>([]);
  const [loading, setLoading] = useState(false);
  const [synthesizing, setSynthesizing] = useState(false);
  const [busyKey, setBusyKey] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);

  const loadDashboard = useCallback(
    async (sensOverride?: boolean) => {
      if (!desktop) return;
      const wantSens = sensOverride ?? includeSensitive;
      setLoading(true);
      setErr(null);
      try {
        const vs = await aegis.vault.status().catch(() => null);
        const unlocked = Boolean(vs?.sensitive_unlocked);
        setSensitiveUnlocked(unlocked);
        const effSens = wantSens && unlocked;
        const [dash, history] = await Promise.all([
          aegis.cdo.dashboard(
            {
              horizon_days: horizonDays,
              include_sensitive: effSens,
              include_dismissed: includeDismissed,
            },
            false
          ),
          aegis.cdo.listBriefings(10),
        ]);
        setDashboard(dash);
        setBriefings(history);
      } catch (e) {
        setErr(errorMessage(e));
      } finally {
        setLoading(false);
      }
    },
    [desktop, horizonDays, includeSensitive, includeDismissed]
  );

  useEffect(() => {
    void loadDashboard();
  }, [loadDashboard]);

  const handleUnlockSensitive = async () => {
    if (!desktop) return;
    setErr(null);
    try {
      await aegis.vault.unlockSensitive();
      setSensitiveUnlocked(true);
      setIncludeSensitive(true);
      await loadDashboard(true);
    } catch (e) {
      setErr(errorMessage(e));
    }
  };

  const handleRatify = async (rec: CdoRecommendation) => {
    if (!desktop) return;
    setBusyKey(rec.rec_key);
    setErr(null);
    setFlash(null);
    try {
      const record = await aegis.cdo.ratifyRecommendation(rec);
      setFlash(`Ratified: ${record.applied_summary} (single-click Undo available below).`);
      await loadDashboard();
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleUndoRatification = async (ratificationId: string) => {
    if (!desktop) return;
    setBusyKey(ratificationId);
    setErr(null);
    setFlash(null);
    try {
      const undone = await aegis.cdo.undoRatification(ratificationId);
      setFlash(`Undone: ${undone.rec_title}.`);
      await loadDashboard();
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleTriage = async (
    recKey: string,
    status: "open" | "snoozed" | "dismissed"
  ) => {
    if (!desktop) return;
    setBusyKey(recKey);
    setErr(null);
    try {
      await aegis.cdo.setRecommendationStatus(recKey, status);
      await loadDashboard();
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusyKey(null);
    }
  };

  const handleSynthesizeBriefing = async () => {
    if (!desktop) return;
    setSynthesizing(true);
    setErr(null);
    setFlash(null);
    try {
      const brief = await aegis.cdo.synthesizeBriefing({
        horizonDays,
        includeSensitive: includeSensitive && sensitiveUnlocked,
        liveProbeBitlocker: false,
      });
      setSubTab("briefings");
      setFlash(`Executive briefing generated with ${brief.model_name}.`);
      await loadDashboard();
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setSynthesizing(false);
    }
  };

  return (
    <div className="space-y-5">
      {/* Executive Header */}
      <div className="flex flex-wrap items-start justify-between gap-4 rounded-xl border border-slate-800 bg-slate-900/60 p-5">
        <div className="space-y-1.5">
          <div className="flex flex-wrap items-center gap-2.5">
            <div className="flex h-9 w-9 items-center justify-center rounded-lg bg-indigo-500/15 text-indigo-300">
              <Gauge className="h-5 w-5" aria-hidden="true" />
            </div>
            <div>
              <h1 className="text-lg font-semibold text-slate-50">
                AI Chief Digital Officer Agent
              </h1>
              <p className="text-xs text-slate-400">{dashboard.posture_summary}</p>
            </div>
          </div>
          <div className="flex flex-wrap items-center gap-2 pt-1 text-xs text-slate-400">
            <span className="inline-flex items-center gap-1 rounded-full border border-emerald-800/50 bg-emerald-950/40 px-2.5 py-0.5 text-emerald-300">
              <ShieldCheck className="h-3 w-3" aria-hidden="true" />
              Reads files on disk only
            </span>
            <span className="inline-flex items-center gap-1 rounded-full border border-slate-700 bg-slate-800/60 px-2.5 py-0.5 text-slate-300">
              You approve · one-click undo
            </span>
            {dashboard.excluded_sensitive_count > 0 && (
              <span className="inline-flex items-center gap-1 rounded-full border border-amber-800/50 bg-amber-950/40 px-2.5 py-0.5 text-amber-300">
                <Lock className="h-3 w-3" aria-hidden="true" />
                {dashboard.excluded_sensitive_count} sensitive files hidden
              </span>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-2">
          <div className="rounded-lg border border-slate-800 bg-slate-950/70 px-3 py-1.5 text-right">
            <div className="text-xs uppercase tracking-wide text-slate-500">
              Executive Index
            </div>
            <div className="text-lg font-bold text-indigo-300">
              {dashboard.executive_score.toFixed(1)}{" "}
              <span className="text-xs font-medium text-slate-400">
                ({dashboard.executive_grade})
              </span>
            </div>
          </div>

          <select
            aria-label="Analysis horizon"
            value={horizonDays}
            onChange={(e) => setHorizonDays(Number(e.target.value))}
            className="rounded-lg border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-xs text-slate-200"
          >
            <option value={7}>Last 7 days</option>
            <option value={30}>Last 30 days</option>
            <option value={90}>Last 90 days</option>
          </select>

          {!sensitiveUnlocked ? (
            <button
              type="button"
              onClick={() => void handleUnlockSensitive()}
              className="inline-flex items-center gap-1.5 rounded-lg border border-amber-700/60 bg-amber-950/40 px-3 py-1.5 text-xs font-medium text-amber-200 hover:bg-amber-900/50"
            >
              <Unlock className="h-3.5 w-3.5" aria-hidden="true" />
              Unlock sensitive files
            </button>
          ) : (
            <label className="inline-flex cursor-pointer items-center gap-1.5 rounded-lg border border-emerald-800/50 bg-emerald-950/30 px-2.5 py-1.5 text-xs text-emerald-200">
              <input
                type="checkbox"
                checked={includeSensitive}
                onChange={(e) => setIncludeSensitive(e.target.checked)}
                className="accent-emerald-400"
              />
              Include Sensitive
            </label>
          )}

          <button
            type="button"
            disabled={loading}
            onClick={() => void loadDashboard()}
            className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700 disabled:opacity-50"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
            Refresh
          </button>

          <button
            type="button"
            disabled={synthesizing || !desktop}
            onClick={() => void handleSynthesizeBriefing()}
            className="inline-flex items-center gap-1.5 rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
          >
            <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
            {synthesizing ? "Synthesizing..." : "Synthesize Executive Briefing"}
          </button>
        </div>
      </div>

      {err && (
        <div className="rounded-lg border border-rose-800/70 bg-rose-950/50 px-4 py-2.5 text-xs text-rose-200">
          {err}
        </div>
      )}
      {flash && (
        <div className="rounded-lg border border-emerald-800/60 bg-emerald-950/40 px-4 py-2.5 text-xs text-emerald-200">
          {flash}
        </div>
      )}

      {/* Five Executive Pillars */}
      <div className="grid gap-3 md:grid-cols-5">
        {dashboard.pillars.map((pillar) => (
          <button
            key={pillar.id}
            type="button"
            onClick={() => onNavigate(pillar.target_screen)}
            className={`group rounded-xl border p-3.5 text-left transition hover:brightness-110 ${pillarStyle(
              pillar.status
            )}`}
          >
            <div className="flex items-center justify-between gap-2 text-xs font-medium">
              <span className="truncate">{pillar.label}</span>
              <ArrowUpRight className="h-3.5 w-3.5 opacity-60 group-hover:opacity-100" />
            </div>
            <div className="mt-1.5 text-xl font-bold text-slate-100">
              {pillar.score.toFixed(1)}
              <span className="ml-1 text-xs font-normal text-slate-400">
                / 100 · {Math.round(pillar.weight * 100)}%
              </span>
            </div>
            <div className="mt-1 line-clamp-2 text-xs text-slate-400">{pillar.detail}</div>
          </button>
        ))}
      </div>

      {/* Sub-Tabs */}
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 pb-2">
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={() => setSubTab("recommendations")}
            className={`rounded-lg px-3 py-1.5 text-xs font-medium transition ${
              subTab === "recommendations"
                ? "bg-indigo-600/20 text-indigo-300 border border-indigo-500/40"
                : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
            }`}
          >
            Strategic Proposals ({dashboard.recommendations.length})
          </button>
          <button
            type="button"
            onClick={() => setSubTab("bottlenecks")}
            className={`rounded-lg px-3 py-1.5 text-xs font-medium transition ${
              subTab === "bottlenecks"
                ? "bg-indigo-600/20 text-indigo-300 border border-indigo-500/40"
                : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
            }`}
          >
            Bottlenecks & Project Pulse ({dashboard.bottlenecks.length})
          </button>
          <button
            type="button"
            onClick={() => setSubTab("briefings")}
            className={`rounded-lg px-3 py-1.5 text-xs font-medium transition ${
              subTab === "briefings"
                ? "bg-indigo-600/20 text-indigo-300 border border-indigo-500/40"
                : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
            }`}
          >
            Executive Briefings ({briefings.length || (dashboard.latest_briefing ? 1 : 0)})
          </button>
        </div>

        <label className="inline-flex cursor-pointer items-center gap-1.5 text-xs text-slate-400">
          <input
            type="checkbox"
            checked={includeDismissed}
            onChange={(e) => setIncludeDismissed(e.target.checked)}
            className="accent-indigo-500"
          />
          Show dismissed
        </label>
      </div>

      {/* Tab 1: Strategic Recommendations + Ratification History */}
      {subTab === "recommendations" && (
        <div className="space-y-4">
          {dashboard.recommendations.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-8 text-center text-xs text-slate-400">
              No active CDO recommendations right now — all monitored pillars look balanced.
            </div>
          ) : (
            <div className="space-y-3">
              {dashboard.recommendations.map((rec) => {
                const isBusy = busyKey === rec.rec_key;
                return (
                  <div
                    key={rec.rec_key}
                    className="rounded-xl border border-slate-800 bg-slate-900/60 p-4"
                  >
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="space-y-1 flex-1 min-w-[260px]">
                        <div className="flex flex-wrap items-center gap-2">
                          {severityBadge(rec.severity)}
                          <span className="rounded bg-slate-800 px-2 py-0.5 text-xs uppercase tracking-wider text-slate-300">
                            {rec.status}
                          </span>
                          <h3 className="text-sm font-semibold text-slate-100">{rec.title}</h3>
                        </div>
                        <p className="text-xs text-slate-300">{rec.rationale}</p>
                        {renderProposalPreview(rec)}
                      </div>

                      <div className="flex flex-wrap items-center gap-2">
                        {rec.proposal.type === "navigate_to_screen" ? (
                          <button
                            type="button"
                            onClick={() =>
                              rec.proposal.type === "navigate_to_screen" &&
                              onNavigate(rec.proposal.screen)
                            }
                            className="inline-flex items-center gap-1 rounded-lg bg-slate-800 px-3 py-1.5 text-xs font-medium text-indigo-300 hover:bg-slate-700"
                          >
                            {rec.proposal.label}
                            <ArrowUpRight className="h-3.5 w-3.5" />
                          </button>
                        ) : (
                          rec.status !== "ratified" && (
                            <button
                              type="button"
                              disabled={!desktop || isBusy}
                              onClick={() => void handleRatify(rec)}
                              className="inline-flex items-center gap-1 rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-50"
                            >
                              <CheckCircle2 className="h-3.5 w-3.5" />
                              Approve proposal
                            </button>
                          )
                        )}

                        {rec.status === "open" && (
                          <>
                            <button
                              type="button"
                              disabled={!desktop || isBusy}
                              onClick={() => void handleTriage(rec.rec_key, "snoozed")}
                              className="inline-flex items-center gap-1 rounded-lg border border-slate-700 px-2.5 py-1.5 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
                            >
                              <Clock className="h-3.5 w-3.5" />
                              Snooze
                            </button>
                            <button
                              type="button"
                              disabled={!desktop || isBusy}
                              onClick={() => void handleTriage(rec.rec_key, "dismissed")}
                              className="inline-flex items-center gap-1 rounded-lg border border-slate-700 px-2.5 py-1.5 text-xs text-slate-400 hover:bg-slate-800 transition-colors"
                            >
                              <EyeOff className="h-3.5 w-3.5" />
                              Dismiss
                            </button>
                          </>
                        )}

                        {(rec.status === "snoozed" || rec.status === "dismissed") && (
                          <button
                            type="button"
                            disabled={!desktop || isBusy}
                            onClick={() => void handleTriage(rec.rec_key, "open")}
                            className="inline-flex items-center gap-1 rounded-lg border border-slate-700 px-2.5 py-1.5 text-xs text-slate-200 hover:bg-slate-800 transition-colors"
                          >
                            <RotateCcw className="h-3.5 w-3.5" />
                            Reopen
                          </button>
                        )}
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {/* Recent Ratifications & Single-Click Undo */}
          <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <div className="mb-3 flex items-center gap-2 text-xs font-semibold text-slate-200">
              <History className="h-4 w-4 text-indigo-400" aria-hidden="true" />
              Recently approved · undo
            </div>
            {dashboard.recent_ratifications.length === 0 ? (
              <p className="text-xs text-slate-500">
                No CDO proposals ratified yet. Every ratified rule or schedule adjustment can be
                undone here with one click.
              </p>
            ) : (
              <div className="space-y-2">
                {dashboard.recent_ratifications.map((r) => (
                  <div
                    key={r.id}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800/80 bg-slate-950/50 px-3 py-2 text-xs"
                  >
                    <div>
                      <div className="font-medium text-slate-200">{r.applied_summary}</div>
                      <div className="text-xs text-slate-500">
                        {r.action_kind} · {formatDateTime(r.created_at)} · Status: {r.status}
                      </div>
                    </div>
                    {r.can_undo ? (
                      <button
                        type="button"
                        disabled={!desktop || busyKey === r.id}
                        onClick={() => void handleUndoRatification(r.id)}
                        className="inline-flex items-center gap-1 rounded-lg border border-amber-700/60 bg-amber-950/30 px-2.5 py-1 text-xs font-medium text-amber-200 hover:bg-amber-900/40"
                      >
                        <RotateCcw className="h-3 w-3" />
                        Undo
                      </button>
                    ) : (
                      <span className="inline-flex items-center gap-1 text-xs text-slate-500">
                        <XCircle className="h-3 w-3" /> Undone {formatDateTime(r.undone_at)}
                      </span>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Tab 2: Bottlenecks, Project Pulse & Operational Velocity */}
      {subTab === "bottlenecks" && (
        <div className="space-y-4">
          <div className="grid gap-3 md:grid-cols-4">
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3.5">
              <div className="text-xs text-slate-400">Files Touched (7d / Window)</div>
              <div className="mt-1 text-lg font-semibold text-slate-100">
                {dashboard.velocity.files_touched_7d} / {dashboard.velocity.files_touched_30d}
              </div>
            </div>
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3.5">
              <div className="text-xs text-slate-400">Automation Ratio</div>
              <div className="mt-1 text-lg font-semibold text-indigo-300">
                {Math.round(dashboard.velocity.filing_automation_ratio * 100)}%
              </div>
            </div>
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3.5">
              <div className="text-xs text-slate-400">Loose Inbox / Invoices</div>
              <div className="mt-1 text-lg font-semibold text-amber-300">
                {dashboard.velocity.loose_inbox_files} / {dashboard.velocity.loose_invoice_docs}
              </div>
            </div>
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3.5">
              <div className="text-xs text-slate-400">Open Action Items / Proposals</div>
              <div className="mt-1 text-lg font-semibold text-slate-100">
                {dashboard.velocity.open_action_items} /{" "}
                {dashboard.velocity.pending_knowledge_proposals}
              </div>
            </div>
          </div>

          <div className="space-y-2.5">
            <h2 className="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-slate-400">
              <AlertTriangle className="h-3.5 w-3.5 text-amber-400" />
              Diagnosed Workflow Bottlenecks
            </h2>
            {dashboard.bottlenecks.length === 0 ? (
              <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-6 text-xs text-slate-400">
                No workflow bottlenecks detected across the current horizon.
              </div>
            ) : (
              dashboard.bottlenecks.map((bn) => (
                <div
                  key={bn.id}
                  className="rounded-xl border border-slate-800 bg-slate-900/60 p-4"
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div className="space-y-1">
                      <div className="flex items-center gap-2">
                        {severityBadge(bn.severity)}
                        <span className="text-sm font-medium text-slate-100">{bn.title}</span>
                      </div>
                      <p className="text-xs text-slate-300">{bn.diagnosis}</p>
                      {bn.evidence_paths.length > 0 && (
                        <div className="mt-1.5 flex flex-wrap gap-1.5">
                          {bn.evidence_paths.slice(0, 4).map((p) => (
                            <code
                              key={p}
                              className="rounded bg-slate-950 px-2 py-0.5 text-xs text-slate-400"
                            >
                              {p}
                            </code>
                          ))}
                        </div>
                      )}
                    </div>
                    <button
                      type="button"
                      onClick={() => onNavigate(bn.recommended_screen)}
                      className="inline-flex items-center gap-1 rounded-lg border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800 transition-colors"
                    >
                      Inspect <ArrowUpRight className="h-3.5 w-3.5" />
                    </button>
                  </div>
                </div>
              ))
            )}
          </div>

          <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <h2 className="mb-3 flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-slate-400">
              <Activity className="h-3.5 w-3.5 text-indigo-400" />
              Project Code vs Documentation Pulse
            </h2>
            {dashboard.project_pulses.length === 0 ? (
              <p className="text-xs text-slate-500">No active projects indexed yet.</p>
            ) : (
              <div className="overflow-x-auto">
                <table className="w-full text-left text-xs">
                  <thead className="border-b border-slate-800 text-xs text-slate-400">
                    <tr>
                      <th className="pb-2 pr-3">Project</th>
                      <th className="pb-2 pr-3">Code</th>
                      <th className="pb-2 pr-3">Docs</th>
                      <th className="pb-2 pr-3">Doc Lag</th>
                      <th className="pb-2 pr-3">Open Actions</th>
                      <th className="pb-2">Flag</th>
                    </tr>
                  </thead>
                  <tbody className="divide-y divide-slate-800/70">
                    {dashboard.project_pulses.map((p) => (
                      <tr key={p.collection_id} className="text-slate-300">
                        <td className="py-2 pr-3 font-medium text-slate-100">{p.name}</td>
                        <td className="py-2 pr-3">{p.code_files}</td>
                        <td className="py-2 pr-3">{p.doc_files}</td>
                        <td className="py-2 pr-3">
                          {p.doc_lag_days !== null ? `${p.doc_lag_days}d` : "—"}
                        </td>
                        <td className="py-2 pr-3">{p.open_action_items}</td>
                        <td className="py-2">
                          <span
                            className={`rounded px-2 py-0.5 text-xs font-medium ${
                              p.health_flag === "doc_lagging"
                                ? "bg-amber-950/60 text-amber-300 border border-amber-800/50"
                                : p.health_flag === "aligned"
                                ? "bg-emerald-950/50 text-emerald-300 border border-emerald-800/50"
                                : "bg-slate-800 text-slate-300"
                            }`}
                          >
                            {p.health_flag}
                          </span>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Tab 3: Grounded Executive Briefings (on-demand local model) */}
      {subTab === "briefings" && (
        <div className="space-y-4">
          {(briefings.length > 0
            ? briefings
            : dashboard.latest_briefing
            ? [dashboard.latest_briefing]
            : []
          ).length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-8 text-center">
              <FileText className="mx-auto h-7 w-7 text-slate-500" />
              <div className="mt-2 text-sm font-medium text-slate-200">
                No executive briefing saved yet
              </div>
              <p className="mt-1 text-xs text-slate-400">
                Click &ldquo;Synthesize Executive Briefing&rdquo; above to run the local model (loaded only for this) against your disk metrics and save a cited memo in your encrypted catalog.
              </p>
            </div>
          ) : (
            (briefings.length > 0
              ? briefings
              : dashboard.latest_briefing
              ? [dashboard.latest_briefing]
              : []
            ).map((b) => (
              <div
                key={b.id}
                className="rounded-xl border border-slate-800 bg-slate-900/60 p-5 space-y-3"
              >
                <div className="flex flex-wrap items-start justify-between gap-2">
                  <div>
                    <h3 className="text-sm font-semibold text-slate-100">{b.headline}</h3>
                    <div className="text-xs text-slate-400">
                      {formatDateTime(b.created_at)} · Horizon {b.horizon_days}d · {b.model_name}{" "}
                      ({b.backend})
                    </div>
                  </div>
                  <span className="rounded border border-indigo-800/60 bg-indigo-950/40 px-2.5 py-0.5 text-xs font-medium text-indigo-300">
                    Score {b.executive_score.toFixed(1)}
                  </span>
                </div>
                <pre className="whitespace-pre-wrap rounded-lg border border-slate-800 bg-slate-950/60 p-3.5 font-sans text-xs leading-relaxed text-slate-200">
                  {b.summary_markdown}
                </pre>
                {b.citations.length > 0 && (
                  <div className="space-y-1">
                    <div className="text-xs font-medium text-slate-400">
                      Grounded Citations
                    </div>
                    <div className="grid gap-1.5 md:grid-cols-2">
                      {b.citations.map((c) => (
                        <div
                          key={c.ref_id}
                          className="rounded border border-slate-800/80 bg-slate-950/40 px-2.5 py-1.5 text-xs text-slate-300"
                        >
                          <span className="font-semibold text-indigo-300">[{c.ref_id}]</span>{" "}
                          <span className="font-medium text-slate-200">{c.title}</span> —{" "}
                          <span className="text-slate-400">{c.detail}</span>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            ))
          )}
        </div>
      )}
    </div>
  );
}
