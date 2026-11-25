import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import {
  Archive,
  CheckCircle2,
  Copy,
  FileSearch,
  HardDrive,
  History,
  KeyRound,
  Lock,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  Shield,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Trash2,
  Unlock,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type FileDetail,
  type GovernanceDashboard,
  type GovernanceFinding,
  type GovernanceFindingStatus,
  type GovernanceRuleAction,
  type GovernanceSeverity,
} from "../bindings/client";
import { formatBytes } from "../format";
import { PREVIEW_GOVERNANCE_DASHBOARD } from "./fixtures/governance";

type GovSubTab = "findings" | "vault" | "posture";

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

function severityPill(sev: GovernanceSeverity) {
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
    default:
      return (
        <span className="rounded border border-slate-700 bg-slate-800 px-2 py-0.5 text-xs text-slate-300">
          Low
        </span>
      );
  }
}

export function DataGovernance({
  desktop,
  onReviewBatches,
}: {
  desktop: boolean;
  onReviewBatches: () => void;
}) {
  const [subTab, setSubTab] = useState<GovSubTab>("findings");
  const [dash, setDash] = useState<GovernanceDashboard>(
    PREVIEW_GOVERNANCE_DASHBOARD
  );
  const [statusFilter, setStatusFilter] = useState<string>("open");
  const [severityFilter, setSeverityFilter] = useState<string>("all");
  const [includeSensitive, setIncludeSensitive] = useState(false);

  const [scanning, setScanning] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [ruleName, setRuleName] = useState("");
  const [rulePrefix, setRulePrefix] = useState("");
  const [rulePattern, setRulePattern] = useState("");
  const [ruleAction, setRuleAction] =
    useState<GovernanceRuleAction>("auto_confidential");

  const loadDashboard = useCallback(
    async (sensOverride?: boolean) => {
      if (!desktop) {
        setDash(PREVIEW_GOVERNANCE_DASHBOARD);
        return;
      }
      try {
        const useSens = sensOverride ?? includeSensitive;
        const res = await aegis.governance.dashboard(
          {
            status_filter:
              statusFilter === "all"
                ? null
                : (statusFilter as GovernanceFindingStatus),
            severity_filter:
              severityFilter === "all"
                ? null
                : (severityFilter as GovernanceSeverity),
            include_sensitive: useSens,
          },
          true
        );
        setDash(res);
      } catch (err) {
        setNotice(errorMessage(err));
      }
    },
    [desktop, includeSensitive, severityFilter, statusFilter]
  );

  useEffect(() => {
    void loadDashboard();
  }, [loadDashboard]);

  const filteredFindings = useMemo(() => {
    if (desktop) return dash.findings;
    return dash.findings.filter((f) => {
      if (statusFilter !== "all" && f.status !== statusFilter) return false;
      if (severityFilter !== "all" && f.severity !== severityFilter) return false;
      return true;
    });
  }, [dash.findings, desktop, severityFilter, statusFilter]);

  const handleToggleSensitive = async () => {
    if (!desktop) {
      setIncludeSensitive((v) => !v);
      return;
    }
    if (!includeSensitive) {
      try {
        await aegis.vault.unlockSensitive();
        setIncludeSensitive(true);
        await loadDashboard(true);
      } catch (err) {
        setNotice(errorMessage(err));
      }
    } else {
      setIncludeSensitive(false);
      await loadDashboard(false);
    }
  };

  const handleScan = async () => {
    if (!desktop) {
      setNotice("Preview mode: desktop app scans local indexed non-backup files for PII & secrets.");
      return;
    }
    setScanning(true);
    setNotice(null);
    try {
      const rep = await aegis.governance.scan({
        scope_root: null,
        include_sensitive: includeSensitive,
        force_rescan: true,
        max_files: null,
      });
      setNotice(
        `Scanned ${rep.files_inspected} file(s): ${rep.findings_discovered} new finding(s), ${rep.findings_resolved} resolved (${rep.duration_ms}ms).`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setScanning(false);
    }
  };

  const handleProtectFinding = async (
    finding: GovernanceFinding,
    targetSens: "confidential" | "secret"
  ) => {
    if (!desktop) {
      setNotice(`Preview mode: would label file(s) as ${targetSens} and vault finding ${finding.id}.`);
      return;
    }
    const fileIds = Array.from(new Set(finding.locations.map((l) => l.file_id)));
    setBusyId(finding.id);
    setNotice(null);
    try {
      const batch = await aegis.governance.applyLabels({
        fileIds,
        targetSensitivity: targetSens,
        findingIds: [finding.id],
        reason: `Protect ${finding.finding_type} as ${targetSens}`,
      });
      setNotice(
        `Marked ${batch.file_count} file(s) as ${targetSens} and filed the finding as protected. You can undo this (batch ${batch.id.slice(0, 8)}).`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleStageQuarantine = async (fileIds: string[], key: string) => {
    if (!desktop) {
      setNotice("Preview mode: would stage exposed file(s) for 30-day Quarantine in Simulator.");
      return;
    }
    setBusyId(key);
    setNotice(null);
    try {
      const batchId = await aegis.governance.proposeQuarantine(
        fileIds,
        includeSensitive
      );
      setNotice(
        `Staged 30-day Quarantine batch ${batchId.slice(0, 8)} is ready to review in the Simulator.`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleTriage = async (findingId: string, status: GovernanceFindingStatus) => {
    if (!desktop) return;
    setBusyId(findingId);
    try {
      await aegis.governance.setFindingStatus(findingId, status);
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleUndoLabelBatch = async (batchId: string) => {
    if (!desktop) return;
    setBusyId(batchId);
    setNotice(null);
    try {
      const res = await aegis.governance.undoLabelBatch(batchId);
      setNotice(
        `Restored previous sensitivity labels for ${res.file_count} file(s) (batch ${batchId.slice(0, 8)}).`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleSaveRule = async (e: FormEvent) => {
    e.preventDefault();
    if (!ruleName.trim()) {
      setNotice("Enter a name for the privacy rule.");
      return;
    }
    if (!desktop) {
      setNotice(`Preview mode: would save privacy rule "${ruleName.trim()}".`);
      return;
    }
    setBusyId("save_rule");
    setNotice(null);
    try {
      await aegis.governance.saveRule({
        id: null,
        name: ruleName.trim(),
        path_prefix: rulePrefix.trim() || null,
        filename_pattern: rulePattern.trim() || null,
        rule_action: ruleAction,
        enabled: true,
      });
      setRuleName("");
      setRulePrefix("");
      setRulePattern("");
      setNotice("Saved privacy rule to SQLCipher catalog.");
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleApplyRule = async (ruleId: string) => {
    if (!desktop) return;
    setBusyId(ruleId);
    setNotice(null);
    try {
      const b = await aegis.governance.applyRule(ruleId);
      setNotice(
        `Applied rule across ${b.file_count} matching file(s) (reversible batch ${b.id.slice(0, 8)}).`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleDeleteRule = async (ruleId: string) => {
    if (!desktop) return;
    setBusyId(ruleId);
    try {
      await aegis.governance.deleteRule(ruleId);
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleInspectFile = async (fileId: string) => {
    if (!desktop) return;
    try {
      const detail = await aegis.file.get(fileId);
      setInspected(detail);
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const handleCopyFirewall = async () => {
    try {
      await navigator.clipboard.writeText(dash.posture.firewall_command);
      setNotice("Copied Windows Firewall outbound-block rule command to clipboard.");
    } catch {
      setNotice("Copy failed — copy the PowerShell rule manually.");
    }
  };

  return (
    <div className="space-y-5">
      {/* Header */}
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <ShieldCheck className="h-5 w-5 text-emerald-400" />
            <h1 className="text-lg font-semibold">Personal Data Governance</h1>
            <span className="rounded border border-emerald-800/50 bg-emerald-950/70 px-2 py-0.5 text-xs font-medium text-emerald-300">
              Encrypted catalog · Windows unlock · BitLocker · Offline
            </span>
          </div>
          <p className="mt-1 text-xs text-slate-400">
            Detects exposed PII, payment cards, tax IDs, and API credentials on this PC without
            storing raw secrets. Sensitive documents stay findable in your local index while hiding
            previews and excluding them from AI prompts until you unlock them with Windows.
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
            } transition-colors`}
          >
            {includeSensitive ? (
              <Unlock className="h-3.5 w-3.5 text-amber-400" />
            ) : (
              <Lock className="h-3.5 w-3.5 text-slate-400" />
            )}
            {includeSensitive
              ? "Sensitive files unlocked"
              : "Unlock sensitive files"}
          </button>

          <button
            type="button"
            disabled={scanning}
            onClick={() => void handleScan()}
            className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${scanning ? "animate-spin" : ""}`} />
            {scanning ? "Scanning Files..." : "Scan PII & Secrets"}
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

      {/* KPI Strip */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs uppercase tracking-wide text-slate-400">
            Privacy Posture
          </div>
          <div className="mt-1 text-xl font-semibold text-emerald-300">
            {dash.posture.privacy_score.toFixed(1)}%
          </div>
          <div className="mt-0.5 text-xs text-slate-400">
            {dash.posture.db_encrypted ? "SQLCipher AES-256 active" : "Unencrypted"}
          </div>
        </div>

        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs uppercase tracking-wide text-slate-400">
            Open Secret Findings
          </div>
          <div className="mt-1 text-xl font-semibold text-rose-300">
            {dash.posture.open_findings_count}
          </div>
          <div className="mt-0.5 text-xs text-slate-400">
            {dash.posture.vaulted_findings_count} protected / vaulted
          </div>
        </div>

        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs uppercase tracking-wide text-slate-400">
            Protected Files
          </div>
          <div className="mt-1 text-xl font-semibold text-amber-300">
            {dash.posture.confidential_files_count + dash.posture.secret_files_count}
          </div>
          <div className="mt-0.5 text-xs text-slate-400">
            {dash.posture.confidential_files_count} confidential ·{" "}
            {dash.posture.secret_files_count} secret
          </div>
        </div>

        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-3">
          <div className="text-xs uppercase tracking-wide text-slate-400">
            AI &amp; Net Guardrails
          </div>
          <div className="mt-1 text-xl font-semibold text-sky-300">
            {dash.posture.active_rules_count} rule(s)
          </div>
          <div className="mt-0.5 text-xs text-slate-400">
            Offline · no screen or keystroke capture
          </div>
        </div>
      </div>

      {/* Sub-tabs */}
      <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 pb-2">
        <div className="flex flex-wrap gap-2 text-xs">
          {[
            {
              id: "findings" as const,
              label: `PII & Secret Findings (${filteredFindings.length})`,
              icon: ShieldAlert,
            },
            {
              id: "vault" as const,
              label: `Sensitive Vault & Rules (${dash.sensitive_files.length} / ${dash.rules.length})`,
              icon: KeyRound,
            },
            {
              id: "posture" as const,
              label: "Privacy Posture & BitLocker",
              icon: Shield,
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
                    ? "border border-emerald-700/60 bg-emerald-600/20 text-emerald-300"
                    : "text-slate-400 hover:bg-slate-800/60 hover:text-slate-200"
                }`}
              >
                <Icon className="h-3.5 w-3.5" />
                {t.label}
              </button>
            );
          })}
        </div>

        <button
          type="button"
          onClick={onReviewBatches}
          className="inline-flex items-center gap-1 rounded-lg border border-emerald-700/60 bg-emerald-950/30 px-2.5 py-1.5 text-xs text-emerald-300 hover:bg-emerald-900/40"
        >
          <Sparkles className="h-3.5 w-3.5" />
          Open Simulator
        </button>
      </div>

      {/* Findings Tab */}
      {subTab === "findings" && (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center gap-2 text-xs">
            <select
              value={statusFilter}
              onChange={(e) => setStatusFilter(e.target.value)}
              className="rounded-lg border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-slate-200"
            >
              <option value="open">Status: Open</option>
              <option value="vaulted">Status: Protected / Vaulted</option>
              <option value="accepted_risk">Status: Accepted Risk</option>
              <option value="false_positive">Status: False Positive</option>
              <option value="resolved">Status: Resolved</option>
              <option value="all">Status: All</option>
            </select>

            <select
              value={severityFilter}
              onChange={(e) => setSeverityFilter(e.target.value)}
              className="rounded-lg border border-slate-700 bg-slate-900 px-2.5 py-1.5 text-slate-200"
            >
              <option value="all">Severity: All</option>
              <option value="critical">Severity: Critical</option>
              <option value="high">Severity: High</option>
              <option value="medium">Severity: Medium</option>
            </select>
          </div>

          {filteredFindings.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-6 text-center text-xs text-slate-400">
              No governance findings match the active filter.
            </div>
          ) : (
            filteredFindings.map((f) => {
              const fileIds = Array.from(new Set(f.locations.map((l) => l.file_id)));
              return (
                <div
                  key={f.id}
                  className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4"
                >
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <div>
                      <div className="flex flex-wrap items-center gap-2">
                        {severityPill(f.severity)}
                        <span className="font-semibold text-sm text-slate-100">
                          {f.summary}
                        </span>
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-xs text-slate-300">
                          {f.finding_type} · risk {(f.context_risk_score * 100).toFixed(0)}%
                        </span>
                        <span className="rounded bg-slate-800/80 px-1.5 py-0.5 text-xs text-slate-300">
                          {f.status}
                        </span>
                      </div>
                      <p className="mt-1 text-xs text-slate-400">
                        {f.recommended_action}
                      </p>
                    </div>

                    <div className="flex flex-wrap items-center gap-1.5 text-xs">
                      <button
                        type="button"
                        disabled={busyId === f.id || fileIds.length === 0}
                        onClick={() => void handleProtectFinding(f, "confidential")}
                        className="rounded border border-amber-700/60 bg-amber-950/40 px-2.5 py-1 text-amber-200 hover:bg-amber-900/50 disabled:opacity-40"
                      >
                        Mark Confidential
                      </button>
                      <button
                        type="button"
                        disabled={busyId === f.id || fileIds.length === 0}
                        onClick={() => void handleProtectFinding(f, "secret")}
                        className="rounded border border-rose-700/60 bg-rose-950/40 px-2.5 py-1 text-rose-200 hover:bg-rose-900/50 disabled:opacity-40"
                      >
                        Mark Secret
                      </button>
                      <button
                        type="button"
                        disabled={busyId === f.id || fileIds.length === 0}
                        onClick={() => void handleStageQuarantine(fileIds, f.id)}
                        className="inline-flex items-center gap-1 rounded bg-slate-800 px-2.5 py-1 text-slate-200 hover:bg-slate-700 disabled:opacity-40"
                      >
                        <Archive className="h-3 w-3" /> Quarantine Copy
                      </button>
                      <button
                        type="button"
                        disabled={busyId === f.id}
                        onClick={() => void handleTriage(f.id, "false_positive")}
                        className="rounded border border-slate-700 px-2 py-1 text-slate-400 hover:bg-slate-800"
                      >
                        False Positive
                      </button>
                      <button
                        type="button"
                        disabled={busyId === f.id}
                        onClick={() => void handleTriage(f.id, "accepted_risk")}
                        className="rounded border border-slate-700 px-2 py-1 text-slate-400 hover:bg-slate-800"
                      >
                        Accept Risk
                      </button>
                    </div>
                  </div>

                  <div className="space-y-1.5">
                    {f.locations.map((loc) => (
                      <div
                        key={`${f.id}-${loc.file_id}-${loc.line_number}`}
                        className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-950/60 px-3 py-2 text-xs"
                      >
                        <div className="min-w-0 space-y-0.5">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="font-mono text-slate-200">
                              {loc.file_path}
                            </span>
                            {loc.line_number > 0 && (
                              <span className="text-slate-400">
                                L{loc.line_number}
                              </span>
                            )}
                            <span className="rounded bg-slate-800 px-1.5 py-0.5 text-xs text-slate-300">
                              {loc.sensitivity}
                            </span>
                            {loc.in_atomic_unit && (
                              <span className="rounded border border-sky-800/50 bg-sky-950/50 px-1.5 py-0.5 text-xs text-sky-300">
                                inside code repo
                              </span>
                            )}
                          </div>
                          <div className="font-mono text-xs text-amber-300">
                            {loc.redacted_snippet}
                          </div>
                        </div>

                        <button
                          type="button"
                          disabled={!desktop}
                          onClick={() => void handleInspectFile(loc.file_id)}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-xs hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                        >
                          <FileSearch className="h-3 w-3" /> Inspect
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

      {/* Vault & Rules Tab */}
      {subTab === "vault" && (
        <div className="space-y-5">
          {/* Privacy Rules Composer */}
          <form
            onSubmit={(e) => void handleSaveRule(e)}
            className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4"
          >
            <h2 className="text-sm font-semibold">
              Privacy &amp; AI exclusion rules
            </h2>
            <div className="grid gap-2 sm:grid-cols-4 text-xs">
              <input
                type="text"
                value={ruleName}
                onChange={(e) => setRuleName(e.target.value)}
                placeholder="Rule name (e.g. Lock Tax Folder)"
                className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-slate-200 placeholder:text-slate-500"
              />
              <input
                type="text"
                value={rulePrefix}
                onChange={(e) => setRulePrefix(e.target.value)}
                placeholder="Optional folder prefix (e.g. C:\Users\...\Taxes)"
                className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 font-mono text-slate-200 placeholder:text-slate-500"
              />
              <input
                type="text"
                value={rulePattern}
                onChange={(e) => setRulePattern(e.target.value)}
                placeholder="Filename glob (e.g. *.env*, *passport*)"
                className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 font-mono text-slate-200 placeholder:text-slate-500"
              />
              <div className="flex gap-2">
                <select
                  value={ruleAction}
                  onChange={(e) =>
                    setRuleAction(e.target.value as GovernanceRuleAction)
                  }
                  className="flex-1 rounded-lg border border-slate-700 bg-slate-950 px-2 py-1.5 text-slate-200"
                >
                  <option value="auto_confidential">Label Confidential</option>
                  <option value="auto_secret">Label Secret</option>
                  <option value="exclude_from_ai">Exclude from AI Prompts</option>
                </select>
                <button
                  type="submit"
                  disabled={busyId === "save_rule"}
                  className="inline-flex items-center gap-1 rounded-md bg-emerald-600 px-3 py-1.5 font-semibold text-white hover:bg-emerald-500 disabled:cursor-not-allowed transition-colors"
                >
                  <Plus className="h-3.5 w-3.5" /> Add
                </button>
              </div>
            </div>

            {dash.rules.length > 0 && (
              <div className="space-y-1.5 pt-2">
                {dash.rules.map((r) => (
                  <div
                    key={r.id}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-950/60 px-3 py-2 text-xs"
                  >
                    <div>
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="font-medium text-slate-100">{r.name}</span>
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-xs text-emerald-300">
                          {r.rule_action}
                        </span>
                      </div>
                      <div className="mt-0.5 font-mono text-xs text-slate-400">
                        {r.path_prefix ? `path: ${r.path_prefix}` : "any path"} ·{" "}
                        {r.filename_pattern
                          ? `pattern: ${r.filename_pattern}`
                          : "any file"}
                      </div>
                    </div>
                    <div className="flex items-center gap-1.5">
                      {r.rule_action !== "exclude_from_ai" && (
                        <button
                          type="button"
                          disabled={busyId === r.id}
                          onClick={() => void handleApplyRule(r.id)}
                          className="inline-flex items-center gap-1 rounded bg-slate-800 px-2.5 py-1 text-slate-200 hover:bg-slate-700"
                        >
                          <Play className="h-3 w-3" /> Apply Rule
                        </button>
                      )}
                      <button
                        type="button"
                        disabled={busyId === r.id}
                        onClick={() => void handleDeleteRule(r.id)}
                        className="rounded border border-slate-700 p-1 text-slate-400 hover:bg-slate-800 hover:text-rose-300"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </form>

          {/* Sensitive Files Inventory */}
          <div className="space-y-2 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <h2 className="text-sm font-semibold">
              Protected Sensitive Files (Searchable by Owner, Previews Masked Until CredUI)
            </h2>
            {dash.sensitive_files.length === 0 ? (
              <p className="text-xs text-slate-400">
                No files are currently marked confidential or secret.
              </p>
            ) : (
              <div className="space-y-1.5">
                {dash.sensitive_files.map((sf) => (
                  <div
                    key={sf.file_id}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-950/60 px-3 py-2 text-xs"
                  >
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <span className="font-medium text-slate-100">{sf.name}</span>
                        <span className="rounded bg-amber-950/60 px-1.5 py-0.5 text-xs text-amber-300 border border-amber-800/50">
                          {sf.sensitivity}
                        </span>
                        <span className="text-slate-400">
                          {formatBytes(sf.size_bytes)} · {sf.drive_label}
                        </span>
                      </div>
                      <div className="truncate font-mono text-xs text-slate-400">
                        {sf.path}
                      </div>
                    </div>
                    <button
                      type="button"
                      disabled={!desktop}
                      onClick={() => void handleInspectFile(sf.file_id)}
                      className="rounded border border-slate-700 px-2 py-1 text-xs hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                    >
                      Inspect
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>

          {/* Reversible Classification Batches */}
          <div className="space-y-2 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <h2 className="flex items-center gap-1.5 text-sm font-semibold">
              <History className="h-4 w-4 text-amber-400" />
              Label history · undo
            </h2>
            {dash.label_batches.length === 0 ? (
              <p className="text-xs text-slate-400">
                No sensitivity classification batches recorded yet.
              </p>
            ) : (
              dash.label_batches.map((b) => (
                <div
                  key={b.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-950/60 px-3 py-2 text-xs"
                >
                  <div>
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-slate-200">{b.reason}</span>
                      <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-xs text-slate-300">
                        {b.target_sensitivity} · {b.status}
                      </span>
                    </div>
                    <div className="text-slate-400">
                      {b.file_count} file(s) · {formatDateTime(b.created_at)}
                    </div>
                  </div>
                  {b.can_undo && (
                    <button
                      type="button"
                      disabled={busyId === b.id}
                      onClick={() => void handleUndoLabelBatch(b.id)}
                      className="inline-flex items-center gap-1 rounded bg-amber-700 px-2.5 py-1 font-medium text-white hover:bg-amber-600"
                    >
                      <RotateCcw className="h-3 w-3" /> Undo Labels
                    </button>
                  )}
                </div>
              ))
            )}
          </div>
        </div>
      )}

      {/* Privacy Posture & BitLocker Tab */}
      {subTab === "posture" && (
        <div className="space-y-4">
          {/* Drive BitLocker Status */}
          <div className="space-y-2.5 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <div className="flex items-center gap-2">
              <HardDrive className="h-4 w-4 text-emerald-400" />
              <h2 className="text-sm font-semibold">
                Disk encryption &amp; deletion policy
              </h2>
            </div>
            <p className="text-xs text-slate-300">{dash.posture.shredding_policy}</p>
            <div className="grid gap-2 sm:grid-cols-2">
              {dash.posture.drives_encryption.map((d) => (
                <div
                  key={d.mount_point}
                  className="rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs"
                >
                  <div className="flex items-center justify-between">
                    <span className="font-semibold text-slate-100">
                      {d.mount_point} ({d.label})
                    </span>
                    <span
                      className={`rounded px-2 py-0.5 text-xs font-medium ${
                        d.protection_status === "protected"
                          ? "bg-emerald-950/60 text-emerald-300 border border-emerald-800/50"
                          : "bg-amber-950/60 text-amber-300 border border-amber-800/50"
                      }`}
                    >
                      {d.protection_status}
                    </span>
                  </div>
                  <div className="mt-1 text-slate-300">{d.encryption_method}</div>
                  <div className="mt-0.5 text-xs text-slate-400">
                    {d.recommended_action}
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Zero-Network & Firewall Hardening */}
          <div className="space-y-2.5 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <div className="flex items-center justify-between gap-2">
              <div className="flex items-center gap-2">
                <CheckCircle2 className="h-4 w-4 text-emerald-400" />
                <h2 className="text-sm font-semibold">
                  Offline guarantee (enforced in code)
                </h2>
              </div>
              <button
                type="button"
                onClick={() => void handleCopyFirewall()}
                className="inline-flex items-center gap-1 rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800 transition-colors"
              >
                <Copy className="h-3 w-3" /> Copy Firewall Command
              </button>
            </div>
            <p className="text-xs text-slate-300">{dash.posture.zero_network_status}</p>
            <pre className="overflow-x-auto rounded-lg bg-slate-950 p-2.5 font-mono text-xs text-slate-300">
              {dash.posture.firewall_command}
            </pre>
          </div>

          {/* Opt-In Observation Matrix */}
          <div className="space-y-2 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
            <h2 className="text-sm font-semibold">
              What Cairn observes (all opt-in)
            </h2>
            <div className="space-y-1.5">
              {dash.posture.opt_in_audits.map((a) => (
                <div
                  key={a.id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-950/60 px-3 py-2 text-xs"
                >
                  <div>
                    <div className="flex items-center gap-2">
                      <span className="font-medium text-slate-200">{a.label}</span>
                    </div>
                    <div className="text-xs text-slate-400">{a.detail}</div>
                  </div>
                  <span
                    className={`rounded px-2 py-0.5 text-xs font-medium ${
                      a.state === "opted_in"
                        ? "bg-amber-950/60 text-amber-300 border border-amber-800/50"
                        : "bg-emerald-950/60 text-emerald-300 border border-emerald-800/50"
                    }`}
                  >
                    {a.state}
                  </span>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* File Inspection Modal */}
      {inspected && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4">
          <div className="max-h-[85vh] w-full max-w-2xl space-y-3 overflow-y-auto rounded-2xl border border-slate-700 bg-slate-900 p-5 text-xs">
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
            <div className="break-all font-mono text-xs text-slate-400">
              {inspected.path} · {formatBytes(inspected.size)} · {inspected.sensitivity}
            </div>
            <pre className="whitespace-pre-wrap rounded-lg bg-slate-950 p-3 font-mono text-xs text-slate-300">
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
