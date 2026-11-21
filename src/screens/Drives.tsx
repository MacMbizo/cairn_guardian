import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { DatabaseBackup, FolderPlus, HardDrive, History, Pin, PinOff, RefreshCw, RotateCcw, ShieldAlert, Undo2, Usb } from "lucide-react";
import { aegis, errorMessage, type DriveRole } from "../bindings/client";
import { baseName, driveName, formatBytes, formatDay } from "../format";
import type { DrivesData } from "./useDrives";

const ROLES: { value: DriveRole; label: string }[] = [
  { value: "general", label: "General" },
  { value: "backup", label: "Backup drive" },
  { value: "unassigned", label: "Unassigned" },
];

const card = "rounded-xl border border-slate-800 bg-slate-900/50 p-4";
const btn = "rounded-md border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40";
const primary = "rounded-md bg-emerald-600 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40";

export function DrivesScreen({ data, onReviewBatches }: { data: DrivesData; onReviewBatches: () => void }) {
  const { drives, plan, coverage, sources, decisions, versions, topupPlan, standing, preview, reload } = data;
  const [msg, setMsg] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [roles, setRoles] = useState<Record<string, DriveRole>>({});
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  useEffect(() => {
    if (plan) setRoles(Object.fromEntries(plan.entries.map((e) => [e.volume_id, e.proposed_role])));
  }, [plan]);

  const act = async (fn: () => Promise<string | void>) => {
    if (preview) {
      setMsg("Preview only: run the desktop app to make changes.");
      return;
    }
    setBusy(true);
    try {
      const out = await fn();
      if (out) setMsg(out);
      await reload();
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const backup = drives.find((d) => d.role === "backup");
  const showPlan = !!plan && plan.entries.length > 0 && (!backup || plan.entries.some((e) => e.proposed_role === "unassigned"));
  const halts = topupPlan?.halted_folders ?? [];

  return (
    <div className="space-y-6">
      {msg && (
        <p role="status" className="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-200">
          {msg}
        </p>
      )}

      {showPlan && plan && (
        <section aria-label="Drive plan" className={`${card} border-sky-500/30`}>
          <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <DatabaseBackup className="h-4 w-4 text-sky-300" aria-hidden="true" /> Drive plan
          </h2>
          {!plan.ready ? (
            <p className="text-xs text-slate-400">Catalog every drive first, so Aegis can suggest which one should be the Backup drive.</p>
          ) : (
            <>
              <p className="mb-3 text-xs text-slate-400">
                One drive becomes the Backup drive; the others stay general storage. Recommended: the drive that already holds the most backup copies, so the least data moves.
              </p>
              <table className="w-full text-sm">
                <tbody>
                  {plan.entries.map((e) => (
                    <tr key={e.volume_id} className="border-t border-slate-800">
                      <td className="py-2">{driveName(e.label, e.volume_id)}</td>
                      <td className="py-2">
                        <select
                          aria-label={`Role for ${driveName(e.label, e.volume_id)}`}
                          value={roles[e.volume_id] ?? e.proposed_role}
                          onChange={(ev) => setRoles({ ...roles, [e.volume_id]: ev.target.value as DriveRole })}
                          className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-xs"
                        >
                          {ROLES.map((r) => (
                            <option key={r.value} value={r.value}>
                              {r.label}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td className="py-2 text-xs text-slate-400">
                        {formatBytes(e.used_bytes)} used
                        {e.volume_id === plan.recommended_backup && ` · holds ${Math.round(e.backup_copy_share * 100)}% of your backup copies already`}
                        {e.duplicate_bytes > 0 && e.volume_id !== plan.recommended_backup && ` · ${formatBytes(e.duplicate_bytes)} duplicates found`}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
              <div className="mt-3 flex gap-2">
                <button
                  type="button"
                  disabled={busy || Object.values(roles).filter((r) => r === "backup").length > 1}
                  className={primary}
                  onClick={() =>
                    void act(async () => {
                      const out = await aegis.drives.applyPlan(Object.entries(roles) as [string, DriveRole][]);
                      const parts = ["Drive plan saved."];
                      if (out.adopt_batch || out.stray_batch) parts.push("Copies to move onto the Backup drive are waiting for your approval in Simulator & Actions.");
                      if (out.only_copies_on_backup > 0)
                        parts.push(`${out.only_copies_on_backup} files exist only on the Backup drive itself and aren't protected yet.`);
                      return parts.join(" ");
                    })
                  }
                >
                  Approve plan
                </button>
                {Object.values(roles).filter((r) => r === "backup").length > 1 && (
                  <span className="text-xs text-amber-300">Choose only one Backup drive.</span>
                )}
              </div>
            </>
          )}
        </section>
      )}

      <section aria-label="Drives" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <HardDrive className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Drives
        </h2>
        {drives.length === 0 ? (
          <p className="text-xs text-slate-400">
            <Usb className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />
            Plug in an external drive. Aegis gives it a small passport file so it recognises it next time, under any drive letter.
          </p>
        ) : (
          <ul className="divide-y divide-slate-800">
            {drives.map((d) => {
              const online = d.status === "online";
              const catalogued = d.last_catalog_complete_at != null;
              return (
                <li key={d.id} className="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className={`h-2 w-2 rounded-full ${online ? "bg-emerald-400" : "bg-slate-600"}`} aria-hidden="true" />
                      <span className="font-medium">{driveName(d.label, d.mount_point)}</span>
                      <span className="text-xs text-slate-400">{online ? d.mount_point : `unplugged, last seen ${formatDay(d.last_seen)}`}</span>
                    </div>
                    <div className="text-xs text-slate-400">
                      {ROLES.find((r) => r.value === d.role)?.label ?? d.role}
                      {online && d.total_bytes > 0 && ` · ${formatBytes(d.total_bytes - d.free_bytes)} of ${formatBytes(d.total_bytes)} used`}
                      {catalogued ? ` · catalogued ${formatDay(d.last_catalog_complete_at ?? 0)}` : " · not catalogued yet"}
                    </div>
                  </div>
                  <div className="flex gap-2">
                    {online && !catalogued && (
                      <button type="button" disabled={busy} className={primary} onClick={() => void act(async () => { await aegis.drives.catalog(d.id); return `Cataloguing ${driveName(d.label, d.mount_point)} in the background. You can unplug it any time; it carries on next time.`; })}>
                        Catalog
                      </button>
                    )}
                    {online && catalogued && (
                      <button
                        type="button"
                        disabled={busy}
                        className={btn}
                        onClick={() =>
                          void act(async () => {
                            const r = await aegis.drives.refresh(d.id);
                            return `${driveName(d.label, d.mount_point)} is up to date (${r.mode === "usn" ? "change journal" : "quick check"}): ${r.created} new, ${r.modified} changed, ${r.deleted} removed.`;
                          })
                        }
                      >
                        <RefreshCw className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />
                        Check for changes
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      {backup && (
        <section aria-label="Backup" className={card}>
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
            <h2 className="flex items-center gap-2 text-sm font-semibold">
              <DatabaseBackup className="h-4 w-4 text-sky-300" aria-hidden="true" /> Backup drive: {driveName(backup.label, backup.mount_point)}
              {coverage && coverage.capacity_bytes > 0 && (
                <span className="text-xs font-normal text-slate-400">
                  {formatBytes(coverage.used_bytes)} of {formatBytes(coverage.capacity_bytes)}
                </span>
              )}
            </h2>
            <div className="flex items-center gap-2">
              <span className="text-xs text-slate-400">
                {standing ? "Backs up automatically when plugged in" : "Backs up only when you click"}
              </span>
              {!standing && (
                <button type="button" disabled={busy} className={btn} onClick={() => void act(async () => { await aegis.backup.grantStanding(); return "From now on the backup tops itself up whenever this drive is plugged in."; })}>
                  Make it automatic
                </button>
              )}
              <button type="button" disabled={busy || backup.status !== "online"} className={primary} onClick={() => void act(async () => { await aegis.backup.start("user_approved"); return "Backing up…"; })}>
                Back up now
              </button>
            </div>
          </div>

          {halts.map((h) => (
            <div key={h.source_folder} className="mb-3 flex flex-wrap items-center justify-between gap-2 rounded-lg border border-amber-500/40 bg-amber-950/20 p-3 text-xs">
              <span className="flex items-center gap-2 text-amber-100">
                <ShieldAlert className="h-4 w-4 text-amber-300" aria-hidden="true" />
                An unusual number of files changed in {baseName(h.source_folder)} ({h.changed} of {h.total}
                {h.suspect_encrypted > 0 ? `, ${h.suspect_encrypted} look scrambled` : ""}). Back them up anyway?
              </span>
              <button type="button" disabled={busy} className={btn} onClick={() => void act(async () => { await aegis.backup.overrideGuard(h.source_folder); return "Those changes will be backed up on the next run."; })}>
                Back them up anyway
              </button>
            </div>
          ))}

          {coverage && (
            <ul className="space-y-2">
              {coverage.tiers.map((t) => {
                const total = t.protected_bytes + t.unprotected_bytes;
                const pct = total > 0 ? (t.protected_bytes / total) * 100 : 100;
                const skipped = t.tier === 0;
                return (
                  <li key={t.tier} className="grid grid-cols-[1.25rem_12rem_1fr_9rem] items-center gap-2 text-xs">
                    <span className={skipped ? "text-slate-500" : t.unprotected_bytes > 0 ? "text-amber-300" : "text-emerald-300"}>
                      {skipped ? "✗" : t.unprotected_bytes > 0 ? "◐" : "✓"}
                    </span>
                    <span className="text-slate-200">{t.label}</span>
                    <span className="h-2 overflow-hidden rounded bg-slate-800">
                      {!skipped && <span className="block h-full bg-sky-400" style={{ width: `${pct}%` }} />}
                    </span>
                    <span className="text-right text-slate-400">
                      {skipped ? `${formatBytes(total)} skipped` : t.unprotected_bytes > 0 ? `${formatBytes(t.unprotected_bytes)} not yet` : formatBytes(t.protected_bytes)}
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
          {coverage?.suggestion && <p className="mt-3 text-xs text-amber-200">{coverage.suggestion}</p>}
        </section>
      )}

      <section aria-label="Backed-up folders" className={card}>
        <div className="mb-3 flex items-center justify-between">
          <h2 className="text-sm font-semibold">Backed-up folders</h2>
          <button
            type="button"
            disabled={busy}
            className={btn}
            onClick={() =>
              void act(async () => {
                const picked = await openDialog({ directory: true, multiple: false, title: "Choose a folder to back up" });
                if (typeof picked !== "string") return;
                await aegis.backup.addSource(picked, false);
                return `${baseName(picked)} will be included in the next backup.`;
              })
            }
          >
            <FolderPlus className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Add folder
          </button>
        </div>
        {sources.length === 0 ? (
          <p className="text-xs text-slate-400">Folders appear here once a Backup drive is set up and copies are placed on it.</p>
        ) : (
          <ul className="divide-y divide-slate-800">
            {sources.map((s) => (
              <li key={s.id} className="flex flex-wrap items-center justify-between gap-2 py-2 text-sm">
                <span className="min-w-0">
                  <span className="font-mono text-xs text-slate-200">{s.folder}</span>
                  {s.pinned && <span className="ml-2 rounded bg-sky-500/15 px-1.5 py-0.5 text-[11px] text-sky-300">Always back up</span>}
                </span>
                <span className="flex gap-2">
                  <button type="button" disabled={busy} className={btn} onClick={() => void act(async () => { await aegis.backup.addSource(s.folder, !s.pinned); })}>
                    {s.pinned ? <PinOff className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> : <Pin className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />}
                    {s.pinned ? "Normal priority" : "Always back up"}
                  </button>
                  {confirmRemove === s.id ? (
                    <span className="flex items-center gap-2 text-xs text-amber-200">
                      Stop backing up {baseName(s.folder)}? Its backup is kept for 90 days, then removed.
                      <button
                        type="button"
                        disabled={busy}
                        className="rounded-md bg-rose-700 px-2 py-0.5 text-white hover:bg-rose-600"
                        onClick={() =>
                          void act(async () => {
                            setConfirmRemove(null);
                            const bytes = await aegis.backup.removeSource(s.id);
                            return `Stopped backing up ${baseName(s.folder)}. Its ${formatBytes(bytes)} backup is kept for 90 days, then removed.`;
                          })
                        }
                      >
                        Stop
                      </button>
                      <button type="button" className={btn} onClick={() => setConfirmRemove(null)}>
                        Keep
                      </button>
                    </span>
                  ) : (
                    <button type="button" disabled={busy} className="rounded-md px-2 py-1 text-xs text-rose-300 hover:bg-rose-950/40" onClick={() => setConfirmRemove(s.id)}>
                      Stop backing up
                    </button>
                  )}
                </span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section aria-label="Your answers" className={card}>
        <h2 className="mb-3 text-sm font-semibold">Duplicate answers Aegis remembers</h2>
        {decisions.length === 0 ? (
          <p className="text-xs text-slate-400">When you answer a duplicate question, Aegis remembers it here and never asks again.</p>
        ) : (
          <ul className="divide-y divide-slate-800">
            {decisions.map((d) => (
              <li key={d.id} className="flex items-center justify-between gap-2 py-2 text-xs">
                <span className="text-slate-300">
                  <strong className="text-slate-100">{d.decision === "keep_both" ? "Keep both" : d.decision === "backup" ? "It's a backup" : "Delete the copy"}</strong>{" "}
                  · {driveName(d.a_drive, "PC")}: {d.a_rel_path || "(whole drive)"} ↔ {driveName(d.b_drive, "PC")}: {d.b_rel_path || "(whole drive)"} · {formatDay(d.decided_at)}
                </span>
                <button type="button" disabled={busy} className={btn} onClick={() => void act(async () => { await aegis.dupes.undoDecision(d.id); return "Aegis will ask about that one again."; })}>
                  <Undo2 className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Undo
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {backup && (
        <section aria-label="Previous versions" className={card}>
          <h2 className="mb-1 flex items-center gap-2 text-sm font-semibold">
            <History className="h-4 w-4 text-slate-300" aria-hidden="true" /> Previous versions
          </h2>
          <p className="mb-3 text-xs text-slate-400">Files you changed or deleted stay on the Backup drive for 90 days. Restoring never overwrites the current file.</p>
          {versions.length === 0 ? (
            <p className="text-xs text-slate-400">None yet.</p>
          ) : (
            <ul className="divide-y divide-slate-800">
              {versions.map((v) => (
                <li key={v.id} className="flex items-center justify-between gap-2 py-2 text-xs">
                  <span className="min-w-0 text-slate-300">
                    <span className="font-medium text-slate-100">{baseName(v.original_rel_path)}</span> · {v.reason === "changed" ? "older version" : v.reason === "deleted" ? "deleted" : "no longer backed up"} ·{" "}
                    {formatDay(v.moved_at)} · kept until {formatDay(v.prune_after)}
                  </span>
                  <button
                    type="button"
                    disabled={busy || backup.status !== "online"}
                    className={btn}
                    onClick={() => void act(async () => `Restored to ${await aegis.backup.restoreVersion(v.id)}`)}
                  >
                    <RotateCcw className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Restore
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      )}

      <p className="text-xs text-slate-500">
        Moves and deletes always wait for your approval.{" "}
        <button type="button" onClick={onReviewBatches} className="text-emerald-300 hover:underline">
          Review waiting batches
        </button>
      </p>
    </div>
  );
}
