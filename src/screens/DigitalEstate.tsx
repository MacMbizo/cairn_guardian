import { useCallback, useEffect, useState, type FormEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  CheckCircle2,
  Copy,
  FileKey2,
  FolderOutput,
  HardDrive,
  HeartHandshake,
  KeyRound,
  Lock,
  Plus,
  RefreshCw,
  ScrollText,
  ShieldCheck,
  Trash2,
  Unlock,
  Users,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type EstateContinuityExport,
  type EstateDashboard,
  type EstatePacketKind,
  type EstateShardSetBundle,
  type EstateShardVerifyResult,
  type TrusteeRole,
} from "../bindings/client";
import { PREVIEW_ESTATE_DASHBOARD } from "./fixtures/estate";

type EstateSubTab = "trustees" | "packets" | "heartbeat" | "export";

function formatDateTime(ms: number | null): string {
  if (!ms) return "—";
  return new Date(ms).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function heartbeatPill(state: EstateDashboard["heartbeat"]["state"]) {
  switch (state) {
    case "active":
      return (
        <span className="rounded-full border border-emerald-800/60 bg-emerald-950/50 px-2.5 py-0.5 text-xs font-medium text-emerald-300">
          Active
        </span>
      );
    case "warning":
      return (
        <span className="rounded-full border border-amber-800/60 bg-amber-950/50 px-2.5 py-0.5 text-xs font-medium text-amber-300">
          Check-in Due Soon
        </span>
      );
    case "grace_period":
      return (
        <span className="rounded-full border border-amber-700/70 bg-amber-950/70 px-2.5 py-0.5 text-xs font-medium text-amber-200">
          Grace Period
        </span>
      );
    case "ready_for_release":
      return (
        <span className="rounded-full border border-sky-700/70 bg-sky-950/60 px-2.5 py-0.5 text-xs font-medium text-sky-200">
          Ready to release (nothing deleted)
        </span>
      );
    default:
      return (
        <span className="rounded-full border border-slate-700 bg-slate-800 px-2.5 py-0.5 text-xs font-medium text-slate-300">
          Disabled
        </span>
      );
  }
}

export function DigitalEstate({ desktop }: { desktop: boolean }) {
  const [subTab, setSubTab] = useState<EstateSubTab>("trustees");
  const [dashboard, setDashboard] = useState<EstateDashboard>(PREVIEW_ESTATE_DASHBOARD);
  const [sensitiveUnlocked, setSensitiveUnlocked] = useState(false);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);

  // Trustee form
  const [tName, setTName] = useState("");
  const [tRel, setTRel] = useState("");
  const [tContact, setTContact] = useState("");
  const [tRole, setTRole] = useState<TrusteeRole>("co_trustee");
  const [tNotes, setTNotes] = useState("");

  // Shamir generator + one-time display + quorum verifier
  const [thresholdK, setThresholdK] = useState(2);
  const [totalN, setTotalN] = useState(3);
  const [generatedBundle, setGeneratedBundle] = useState<EstateShardSetBundle | null>(null);
  const [quorumInput, setQuorumInput] = useState("");
  const [verifyResult, setVerifyResult] = useState<EstateShardVerifyResult | null>(null);

  // Packet form
  const [pTitle, setPTitle] = useState("");
  const [pKind, setPKind] = useState<EstatePacketKind>("letter");
  const [pTrusteeId, setPTrusteeId] = useState("");
  const [pBody, setPBody] = useState("");
  const [pIncludeBackup, setPIncludeBackup] = useState(true);
  const [pIncludeProjects, setPIncludeProjects] = useState(true);

  // Heartbeat form
  const [hbEnabled, setHbEnabled] = useState(true);
  const [hbInterval, setHbInterval] = useState(30);
  const [hbGrace, setHbGrace] = useState(14);
  const [checkinNote, setCheckinNote] = useState("");

  // Export
  const [exportDir, setExportDir] = useState("");
  const [exportResult, setExportResult] = useState<EstateContinuityExport | null>(null);

  const loadDashboard = useCallback(async () => {
    if (!desktop) return;
    setLoading(true);
    setErr(null);
    try {
      const [vs, dash] = await Promise.all([
        aegis.vault.status().catch(() => null),
        aegis.estate.dashboard(),
      ]);
      setSensitiveUnlocked(Boolean(vs?.sensitive_unlocked));
      setDashboard(dash);
      setHbEnabled(dash.heartbeat.config.enabled);
      setHbInterval(dash.heartbeat.config.interval_days);
      setHbGrace(dash.heartbeat.config.grace_days);
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [desktop]);

  useEffect(() => {
    void loadDashboard();
  }, [loadDashboard]);

  const handleUnlock = async () => {
    if (!desktop) return;
    setErr(null);
    try {
      await aegis.vault.unlockSensitive();
      await loadDashboard();
    } catch (e) {
      setErr(errorMessage(e));
    }
  };

  const handleAddTrustee = async (e: FormEvent) => {
    e.preventDefault();
    if (!desktop || !tName.trim()) return;
    setErr(null);
    try {
      await aegis.estate.saveTrustee({
        id: null,
        name: tName.trim(),
        relationship: tRel.trim(),
        contact_hint: tContact.trim(),
        role: tRole,
        notes: tNotes.trim(),
      });
      setTName("");
      setTRel("");
      setTContact("");
      setTNotes("");
      setFlash("Trustee added to your encrypted succession directory.");
      await loadDashboard();
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  const handleGenerateShards = async () => {
    if (!desktop) return;
    setErr(null);
    setFlash(null);
    try {
      const assigned: (string | null)[] = [];
      for (let i = 0; i < totalN; i++) {
        assigned.push(dashboard.trustees[i]?.id ?? null);
      }
      const bundle = await aegis.estate.generateShards({
        thresholdK,
        totalN,
        trusteeIds: assigned,
      });
      setGeneratedBundle(bundle);
      setVerifyResult(null);
      setFlash(
        `Generated ${bundle.threshold_k}-of-${bundle.total_n} Shamir shards (${bundle.shard_set_id}). Copy them now — raw shards are never saved to aegis.db.`
      );
      await loadDashboard();
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  const handleVerifyQuorum = async () => {
    if (!desktop) return;
    setErr(null);
    const lines = quorumInput
      .split(/\r?\n/)
      .map((s) => s.trim())
      .filter(Boolean);
    try {
      const res = await aegis.estate.verifyQuorum(lines);
      setVerifyResult(res);
      await loadDashboard();
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  const handleSavePacket = async (e: FormEvent) => {
    e.preventDefault();
    if (!desktop || !pTitle.trim()) return;
    setErr(null);
    try {
      await aegis.estate.savePacket({
        id: null,
        title: pTitle.trim(),
        trustee_id: pTrusteeId || null,
        packet_kind: pKind,
        body_markdown: pBody,
        include_backup_map: pIncludeBackup,
        include_project_inventory: pIncludeProjects,
      });
      setPTitle("");
      setPBody("");
      setFlash("Succession packet saved inside SQLCipher vault.");
      await loadDashboard();
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  const handleSaveHeartbeat = async (e: FormEvent) => {
    e.preventDefault();
    if (!desktop) return;
    setErr(null);
    try {
      await aegis.estate.saveHeartbeat({
        enabled: hbEnabled,
        intervalDays: hbInterval,
        graceDays: hbGrace,
      });
      setFlash("Check-in schedule updated.");
      await loadDashboard();
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  const handleCheckin = async () => {
    if (!desktop) return;
    setErr(null);
    try {
      await aegis.estate.checkin("manual", checkinNote.trim() || null);
      setCheckinNote("");
      setFlash("Check-in recorded and countdown reset.");
      await loadDashboard();
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  const handlePickExportFolder = async () => {
    try {
      const picked = await openDialog({ directory: true, multiple: false });
      if (typeof picked === "string") setExportDir(picked);
    } catch {
      // manual input works as fallback
    }
  };

  const handleExportKit = async () => {
    if (!desktop || !exportDir.trim()) return;
    setErr(null);
    try {
      const res = await aegis.estate.exportContinuityKit(exportDir.trim());
      setExportResult(res);
      setFlash(`Exported ${res.files_written.length} Continuity Kit files cleanly.`);
    } catch (ex) {
      setErr(errorMessage(ex));
    }
  };

  return (
    <div className="space-y-5">
      {/* Header + no-deletion guarantee */}
      <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-5 space-y-3">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="flex items-start gap-3">
            <div className="flex h-8 w-8 items-center justify-center rounded-lg border border-slate-800 bg-slate-900 text-emerald-400">
              <ScrollText className="h-4 w-4" aria-hidden="true" />
            </div>
            <div>
              <h1 className="text-lg font-semibold text-slate-50">
                Digital Estate
              </h1>
              <p className="text-xs text-slate-400">
                Share your recovery key with people you trust, leave sealed instructions, and set a check-in that can release them. Nothing is ever deleted.
              </p>
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <div className="rounded-lg border border-slate-800 bg-slate-950/70 px-3 py-1.5 text-right">
              <div className="text-xs uppercase tracking-wider text-slate-500">
                Continuity Readiness
              </div>
              <div className="text-lg font-bold text-emerald-300">
                {dashboard.readiness_score.toFixed(0)}%{" "}
                <span className="text-xs font-normal text-slate-400">
                  ({dashboard.readiness_grade})
                </span>
              </div>
            </div>

            {!sensitiveUnlocked ? (
              <button
                type="button"
                onClick={() => void handleUnlock()}
                className="inline-flex items-center gap-1.5 rounded-md border border-amber-700 bg-amber-950/40 px-3 py-1.5 text-xs font-medium text-amber-200 transition-colors hover:bg-amber-900/50"
              >
                <Unlock className="h-3.5 w-3.5" />
                Unlock estate details
              </button>
            ) : (
              <span className="inline-flex items-center gap-1.5 rounded-lg border border-emerald-800/60 bg-emerald-950/40 px-3 py-1.5 text-xs text-emerald-300">
                <ShieldCheck className="h-3.5 w-3.5" />
                Estate details unlocked
              </span>
            )}

            <button
              type="button"
              disabled={loading}
              onClick={() => void loadDashboard()}
              className="inline-flex items-center gap-1.5 rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs text-slate-200 hover:bg-slate-700"
            >
              <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
              Refresh
            </button>
          </div>
        </div>

        <div className="grid gap-2.5 md:grid-cols-3 text-xs">
          <div className="rounded-lg border border-emerald-900/50 bg-emerald-950/20 px-3 py-2 text-emerald-200">
            <span className="font-semibold">Never deletes your data:</span>{" "}
            {dashboard.r10_policy_summary}
          </div>
          <div className="rounded-lg border border-slate-800 bg-slate-950/60 px-3 py-2 text-slate-300">
            <span className="font-semibold text-slate-200">Data at rest:</span>{" "}
            {dashboard.r8_policy_summary}
          </div>
          <div className="rounded-lg border border-slate-800 bg-slate-950/60 px-3 py-2 text-slate-300 flex items-center justify-between">
            <span className="inline-flex items-center gap-1.5">
              <HardDrive className="h-3.5 w-3.5 text-indigo-400" />
              Backup Drives: {dashboard.backup_drives_online} / {dashboard.backup_drives_count} online
            </span>
            <span>Vault: {dashboard.db_encrypted ? "SQLCipher D1" : "Unencrypted"}</span>
          </div>
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

      {/* Sub-Tabs */}
      <div className="flex flex-wrap gap-2 border-b border-slate-800 pb-2">
        {(
          [
            ["trustees", `Trustees & Shards (${dashboard.trustees.length})`, Users],
            ["packets", `Succession Packets (${dashboard.packets.length})`, KeyRound],
            ["heartbeat", "Check-in heartbeat", HeartHandshake],
            ["export", "Continuity Kit Export", FolderOutput],
          ] as const
        ).map(([id, label, Icon]) => (
          <button
            key={id}
            type="button"
            onClick={() => setSubTab(id)}
            className={`inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-xs font-medium transition ${
              subTab === id
                ? "border border-indigo-500/40 bg-indigo-600/20 text-indigo-300"
                : "text-slate-400 hover:bg-slate-900 hover:text-slate-200"
            }`}
          >
            <Icon className="h-3.5 w-3.5" />
            {label}
          </button>
        ))}
      </div>

      {/* Tab 1: Trustees & K-of-N Shamir Shards */}
      {subTab === "trustees" && (
        <div className="grid gap-5 lg:grid-cols-2">
          <div className="space-y-4">
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 space-y-3">
              <h2 className="text-xs font-semibold uppercase tracking-wider text-slate-300">
                Designated Trustees (`D3`)
              </h2>
              {dashboard.trustees.length === 0 ? (
                <p className="text-xs text-slate-500">No trustees registered yet.</p>
              ) : (
                <div className="space-y-2">
                  {dashboard.trustees.map((t) => (
                    <div
                      key={t.id}
                      className="flex items-start justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/50 p-3 text-xs"
                    >
                      <div>
                        <div className="font-semibold text-slate-100">
                          {t.name}{" "}
                          <span className="ml-1 rounded bg-slate-800 px-1.5 py-0.5 text-xs text-indigo-300">
                            {t.role}
                          </span>
                        </div>
                        <div className="text-xs text-slate-400">
                          {t.relationship} · {t.contact_hint}
                        </div>
                        {t.notes && <div className="mt-1 text-xs text-slate-500">{t.notes}</div>}
                      </div>
                      <button
                        type="button"
                        disabled={!desktop}
                        onClick={() => void aegis.estate.deleteTrustee(t.id).then(loadDashboard)}
                        className="text-slate-500 hover:text-rose-400"
                        title="Remove trustee"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                  ))}
                </div>
              )}

              <form onSubmit={(e) => void handleAddTrustee(e)} className="space-y-2 pt-2 border-t border-slate-800">
                <div className="text-xs font-medium text-slate-300">Add Trustee</div>
                <div className="grid gap-2 sm:grid-cols-2">
                  <input
                    value={tName}
                    onChange={(e) => setTName(e.target.value)}
                    placeholder="Full name"
                    className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                  />
                  <select
                    value={tRole}
                    onChange={(e) => setTRole(e.target.value as TrusteeRole)}
                    className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                  >
                    <option value="primary_executor">Primary Executor</option>
                    <option value="co_trustee">Co-Trustee</option>
                    <option value="legal_counsel">Legal Counsel</option>
                    <option value="beneficiary">Beneficiary</option>
                  </select>
                  <input
                    value={tRel}
                    onChange={(e) => setTRel(e.target.value)}
                    placeholder="Relationship (Spouse, Counsel...)"
                    className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                  />
                  <input
                    value={tContact}
                    onChange={(e) => setTContact(e.target.value)}
                    placeholder="Contact hint / vault location"
                    className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                  />
                </div>
                <input
                  value={tNotes}
                  onChange={(e) => setTNotes(e.target.value)}
                  placeholder="Optional handling notes"
                  className="w-full rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                />
                <button
                  type="submit"
                  disabled={!desktop || !tName.trim()}
                  className="inline-flex items-center gap-1 rounded-lg bg-slate-800 px-3 py-1.5 text-xs font-medium text-slate-100 hover:bg-slate-700 disabled:opacity-50"
                >
                  <Plus className="h-3.5 w-3.5" /> Add Trustee
                </button>
              </form>
            </div>
          </div>

          <div className="space-y-4">
            <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 space-y-3">
              <div className="flex items-center justify-between gap-2">
                <h2 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wider text-slate-300">
                  <FileKey2 className="h-4 w-4 text-indigo-400" />
                  Recovery key shares (any K of N)
                </h2>
                <div className="flex items-center gap-2 text-xs">
                  <label className="text-slate-400">
                    K:
                    <input
                      type="number"
                      min={2}
                      max={16}
                      value={thresholdK}
                      onChange={(e) => setThresholdK(Number(e.target.value))}
                      className="ml-1 w-12 rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-slate-100"
                    />
                  </label>
                  <label className="text-slate-400">
                    N:
                    <input
                      type="number"
                      min={2}
                      max={16}
                      value={totalN}
                      onChange={(e) => setTotalN(Number(e.target.value))}
                      className="ml-1 w-12 rounded border border-slate-700 bg-slate-950 px-1.5 py-0.5 text-slate-100"
                    />
                  </label>
                  <button
                    type="button"
                    disabled={!desktop || !sensitiveUnlocked}
                    onClick={() => void handleGenerateShards()}
                    className="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-40"
                  >
                    Generate Shards
                  </button>
                </div>
              </div>

              {generatedBundle && (
                <div className="rounded-lg border border-amber-700/60 bg-amber-950/30 p-3 space-y-2 text-xs">
                  <div className="font-semibold text-amber-200">
                    One-Time Generated Shards ({generatedBundle.threshold_k}-of-
                    {generatedBundle.total_n} · Key `{generatedBundle.key_fingerprint}`)
                  </div>
                  <p className="text-xs text-amber-300/90">
                    Distribute these immediately. Raw shards are NEVER stored in your catalog.
                  </p>
                  <div className="space-y-1.5">
                    {generatedBundle.shards.map((s) => (
                      <div
                        key={s.record_id}
                        className="flex items-center justify-between gap-2 rounded bg-slate-950/80 px-2.5 py-1.5"
                      >
                        <code className="truncate text-xs text-slate-200">
                          #{s.share_index} ({s.trustee_name ?? "Unassigned"}): {s.shard_token}
                        </code>
                        <button
                          type="button"
                          onClick={() => void navigator.clipboard.writeText(s.shard_token)}
                          className="shrink-0 text-indigo-300 hover:text-indigo-200"
                          title="Copy shard"
                        >
                          <Copy className="h-3.5 w-3.5" />
                        </button>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              <div className="space-y-1.5">
                {dashboard.shard_records.slice(0, 8).map((sr) => (
                  <div
                    key={sr.id}
                    className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/50 px-3 py-2 text-xs"
                  >
                    <div>
                      <span className="font-medium text-slate-200">
                        Share #{sr.share_index}/{sr.total_n} (K={sr.threshold_k})
                      </span>{" "}
                      · {sr.trustee_name ?? "Unassigned"} · checksum{" "}
                      <code className="text-slate-400">{sr.checksum_prefix}</code>
                      <div className="text-xs text-slate-500">
                        Set `{sr.shard_set_id}` ·{" "}
                        {sr.revoked_at
                          ? "Revoked"
                          : sr.verified_at
                          ? "Verified"
                          : sr.delivered_at
                          ? "Delivered"
                          : "Issued"}
                      </div>
                    </div>
                    {!sr.revoked_at && !sr.delivered_at && (
                      <button
                        type="button"
                        disabled={!desktop}
                        onClick={() =>
                          void aegis.estate.markShardDelivered(sr.id).then(loadDashboard)
                        }
                        className="rounded border border-slate-700 px-2 py-0.5 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
                      >
                        Mark Delivered
                      </button>
                    )}
                  </div>
                ))}
              </div>

              <div className="border-t border-slate-800 pt-3 space-y-2">
                <div className="text-xs font-medium text-slate-300">
                  Quorum Verification Drill (Paste K+ Shards, one per line)
                </div>
                <textarea
                  rows={3}
                  value={quorumInput}
                  onChange={(e) => setQuorumInput(e.target.value)}
                  placeholder="CAIRN-SHARD-V1:..."
                  className="w-full rounded-lg border border-slate-700 bg-slate-950 p-2 font-mono text-xs text-slate-200"
                />
                <div className="flex items-center justify-between gap-2">
                  <button
                    type="button"
                    disabled={!desktop || !sensitiveUnlocked || !quorumInput.trim()}
                    onClick={() => void handleVerifyQuorum()}
                    className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs text-slate-100 hover:bg-slate-700 disabled:opacity-40"
                  >
                    Verify Quorum Match
                  </button>
                  {verifyResult && (
                    <span
                      className={`text-xs ${
                        verifyResult.valid ? "text-emerald-300" : "text-rose-300"
                      }`}
                    >
                      {verifyResult.detail}
                    </span>
                  )}
                </div>
              </div>
            </div>
          </div>
        </div>
      )}

      {/* Tab 2: Succession Packets */}
      {subTab === "packets" && (
        <div className="grid gap-5 lg:grid-cols-2">
          <div className="space-y-3">
            {dashboard.packets.map((p) => (
              <div
                key={p.id}
                className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 space-y-2"
              >
                <div className="flex items-start justify-between gap-2">
                  <div>
                    <div className="text-sm font-semibold text-slate-100">{p.title}</div>
                    <div className="text-xs text-slate-400">
                      {p.packet_kind} · Trustee: {p.trustee_name ?? "General"}
                    </div>
                  </div>
                  {sensitiveUnlocked && (
                    <button
                      type="button"
                      disabled={!desktop}
                      onClick={() => void aegis.estate.deletePacket(p.id).then(loadDashboard)}
                      className="text-slate-500 hover:text-rose-400"
                    >
                      <Trash2 className="h-3.5 w-3.5" />
                    </button>
                  )}
                </div>
                {p.masked ? (
                  <div className="flex items-center gap-1.5 rounded-lg border border-amber-900/40 bg-amber-950/20 px-3 py-2 text-xs text-amber-300">
                    <Lock className="h-3.5 w-3.5" />
                    {p.body_markdown}
                  </div>
                ) : (
                  <pre className="whitespace-pre-wrap rounded-lg border border-slate-800 bg-slate-950/60 p-3 font-sans text-xs text-slate-200">
                    {p.body_markdown}
                  </pre>
                )}
              </div>
            ))}
          </div>

          <form
            onSubmit={(e) => void handleSavePacket(e)}
            className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 space-y-3"
          >
            <h2 className="text-xs font-semibold uppercase tracking-wider text-slate-300">
              Create sealed succession packet
            </h2>
            <input
              value={pTitle}
              onChange={(e) => setPTitle(e.target.value)}
              placeholder="Packet title (e.g. Hardware Keys & Bank Vault Instructions)"
              className="w-full rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs text-slate-100"
            />
            <div className="grid gap-2 sm:grid-cols-2">
              <select
                value={pKind}
                onChange={(e) => setPKind(e.target.value as EstatePacketKind)}
                className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              >
                <option value="letter">Personal Letter</option>
                <option value="legal_instructions">Legal Instructions</option>
                <option value="account_directory">Account Directory</option>
                <option value="drive_map">Drive & Vault Map</option>
              </select>
              <select
                value={pTrusteeId}
                onChange={(e) => setPTrusteeId(e.target.value)}
                className="rounded-lg border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              >
                <option value="">All Designated Trustees</option>
                {dashboard.trustees.map((t) => (
                  <option key={t.id} value={t.id}>
                    {t.name}
                  </option>
                ))}
              </select>
            </div>
            <textarea
              rows={6}
              value={pBody}
              onChange={(e) => setPBody(e.target.value)}
              placeholder="Write instructions that only unlock after Windows Hello / CredUI authentication..."
              className="w-full rounded-lg border border-slate-700 bg-slate-950 p-2.5 text-xs text-slate-100"
            />
            <div className="flex flex-wrap gap-4 text-xs text-slate-300">
              <label className="inline-flex items-center gap-1.5">
                <input
                  type="checkbox"
                  checked={pIncludeBackup}
                  onChange={(e) => setPIncludeBackup(e.target.checked)}
                />
                Include Backup Drive Map
              </label>
              <label className="inline-flex items-center gap-1.5">
                <input
                  type="checkbox"
                  checked={pIncludeProjects}
                  onChange={(e) => setPIncludeProjects(e.target.checked)}
                />
                Include Active Project Directory
              </label>
            </div>
            <button
              type="submit"
              disabled={!desktop || !sensitiveUnlocked || !pTitle.trim()}
              className="rounded-lg bg-indigo-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-40"
            >
              Save Sealed Packet
            </button>
          </form>
        </div>
      )}

      {/* Tab 3: Inactivity Heartbeat */}
      {subTab === "heartbeat" && (
        <div className="grid gap-5 lg:grid-cols-2">
          <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 space-y-4">
            <div className="flex items-center justify-between">
              <h2 className="text-xs font-semibold uppercase tracking-wider text-slate-300">
                Check-in schedule
              </h2>
              {heartbeatPill(dashboard.heartbeat.state)}
            </div>
            <div className="grid grid-cols-2 gap-3 text-xs">
              <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
                <div className="text-slate-400">Next Check-In Due</div>
                <div className="mt-1 font-medium text-slate-100">
                  {formatDateTime(dashboard.heartbeat.next_due_at)}
                </div>
              </div>
              <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
                <div className="text-slate-400">Days Remaining</div>
                <div className="mt-1 font-medium text-indigo-300">
                  {dashboard.heartbeat.days_remaining ?? "—"}
                </div>
              </div>
            </div>

            <div className="flex gap-2">
              <input
                value={checkinNote}
                onChange={(e) => setCheckinNote(e.target.value)}
                placeholder="Optional check-in note"
                className="flex-1 rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs text-slate-100"
              />
              <button
                type="button"
                disabled={!desktop}
                onClick={() => void handleCheckin()}
                className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-medium text-white hover:bg-emerald-500 disabled:cursor-not-allowed transition-colors"
              >
                <CheckCircle2 className="h-3.5 w-3.5" /> Check In Now
              </button>
            </div>

            <form
              onSubmit={(e) => void handleSaveHeartbeat(e)}
              className="space-y-2 border-t border-slate-800 pt-3 text-xs"
            >
              <label className="inline-flex items-center gap-2 text-slate-200">
                <input
                  type="checkbox"
                  checked={hbEnabled}
                  onChange={(e) => setHbEnabled(e.target.checked)}
                />
                Turn on check-in heartbeat (never deletes anything)
              </label>
              <div className="grid grid-cols-2 gap-3">
                <label className="text-slate-400">
                  Interval (days):
                  <input
                    type="number"
                    min={7}
                    max={365}
                    value={hbInterval}
                    onChange={(e) => setHbInterval(Number(e.target.value))}
                    className="mt-1 w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1 text-slate-100"
                  />
                </label>
                <label className="text-slate-400">
                  Grace Period (days):
                  <input
                    type="number"
                    min={3}
                    max={90}
                    value={hbGrace}
                    onChange={(e) => setHbGrace(Number(e.target.value))}
                    className="mt-1 w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1 text-slate-100"
                  />
                </label>
              </div>
              <button
                type="submit"
                disabled={!desktop}
                className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs text-slate-100 hover:bg-slate-700"
              >
                Save Schedule
              </button>
            </form>
          </div>

          <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-4 space-y-2">
            <h2 className="text-xs font-semibold uppercase tracking-wider text-slate-300">
              Recent Check-Ins
            </h2>
            {dashboard.recent_checkins.length === 0 ? (
              <p className="text-xs text-slate-500">No check-ins recorded yet.</p>
            ) : (
              <div className="space-y-2">
                {dashboard.recent_checkins.map((c) => (
                  <div
                    key={c.id}
                    className="flex items-center justify-between rounded-lg border border-slate-800 bg-slate-950/50 px-3 py-2 text-xs"
                  >
                    <div>
                      <span className="font-medium text-slate-200">{c.method}</span>{" "}
                      {c.note && <span className="text-slate-400">— {c.note}</span>}
                    </div>
                    <span className="text-xs text-slate-500">
                      {formatDateTime(c.checked_in_at)}
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      )}

      {/* Tab 4: Continuity Kit Export */}
      {subTab === "export" && (
        <div className="rounded-xl border border-slate-800 bg-slate-900/60 p-5 space-y-4">
          <div>
            <h2 className="text-sm font-semibold text-slate-100">
              Export offline Continuity Kit for trustees
            </h2>
            <p className="mt-1 text-xs text-slate-400">
              Writes `README_TRUSTEE_RECOVERY.md`, sealed packet Markdown files, and `manifest.json`
              to a chosen destination (such as a Backup drive or USB). Never includes raw Shamir
              shards or `DbKey`, and never deletes existing files.
            </p>
          </div>
          <div className="flex flex-wrap gap-2">
            <input
              value={exportDir}
              onChange={(e) => setExportDir(e.target.value)}
              placeholder="Destination folder path..."
              className="flex-1 min-w-[240px] rounded-lg border border-slate-700 bg-slate-950 px-3 py-1.5 text-xs text-slate-100"
            />
            <button
              type="button"
              onClick={() => void handlePickExportFolder()}
              className="rounded-lg border border-slate-700 bg-slate-800 px-3 py-1.5 text-xs text-slate-200 hover:bg-slate-700"
            >
              Browse...
            </button>
            <button
              type="button"
              disabled={!desktop || !sensitiveUnlocked || !exportDir.trim()}
              onClick={() => void handleExportKit()}
              className="rounded-lg bg-indigo-600 px-4 py-1.5 text-xs font-medium text-white hover:bg-indigo-500 disabled:opacity-40"
            >
              Export Continuity Kit
            </button>
          </div>
          {exportResult && (
            <div className="rounded-lg border border-emerald-800/50 bg-emerald-950/30 p-3 text-xs text-emerald-200 space-y-1">
              <div className="font-semibold">Exported to `{exportResult.output_dir}`</div>
              <ul className="list-disc pl-5 text-xs text-emerald-300/90">
                {exportResult.files_written.map((f) => (
                  <li key={f}>{f}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
