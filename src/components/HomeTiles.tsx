import { CopyCheck, DatabaseBackup, HardDrive, ShieldAlert, Usb } from "lucide-react";
import type { DrivesData } from "../screens/useDrives";
import { driveName, formatBytes, formatDay } from "../format";

export type DrivesTab = "drives" | "dupes";

const ROLE_CHIP: Record<string, string> = {
  backup: "bg-sky-500/15 text-sky-300",
  general: "bg-slate-700/60 text-slate-200",
  unassigned: "bg-amber-500/15 text-amber-300",
};
const ROLE_LABEL: Record<string, string> = {
  backup: "Backup drive",
  general: "General",
  unassigned: "Unassigned",
};

/** Command-centre tiles for the home screen: drives at a glance, and what needs the owner. */
export function HomeTiles({ data, onOpen }: { data: DrivesData; onOpen: (tab: DrivesTab) => void }) {
  const { drives, plan, coverage, dupes, topupPlan, unknownDrives } = data;
  const questions = dupes?.groups.filter((g) => g.needs_question) ?? [];
  const questionBytes = questions.reduce((sum, g) => sum + g.size_bytes * Math.max(0, g.locations.length - 1), 0);
  const halts = topupPlan?.halted_folders ?? [];
  const planPending = !!plan?.ready && !drives.some((d) => d.role === "backup") && drives.length > 0;
  const unprotected = coverage?.tiers.filter((t) => t.tier !== 0).reduce((s, t) => s + t.unprotected_bytes, 0) ?? 0;

  const needs: { key: string; icon: typeof Usb; text: string; action: string; tab: DrivesTab }[] = [];
  if (planPending) needs.push({ key: "plan", icon: DatabaseBackup, text: "Drive plan ready: choose your Backup drive", action: "Review", tab: "drives" });
  if (unknownDrives.length > 0)
    needs.push({ key: "new", icon: Usb, text: `${unknownDrives.length} new drive${unknownDrives.length > 1 ? "s" : ""} plugged in`, action: "Catalog", tab: "drives" });
  if (questions.length > 0)
    needs.push({ key: "dupes", icon: CopyCheck, text: `${questions.length} duplicate ${questions.length > 1 ? "groups" : "group"} · ${formatBytes(questionBytes)}`, action: "Review", tab: "dupes" });
  for (const h of halts)
    needs.push({ key: `halt-${h.source_folder}`, icon: ShieldAlert, text: `Backup paused: ${h.changed} of ${h.total} files changed in ${h.source_folder.split(/[\\/]/).pop()}`, action: "Check", tab: "drives" });

  return (
    <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
      <section aria-label="Drives" className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <HardDrive className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Drives
          </h2>
          <button type="button" onClick={() => onOpen("drives")} className="text-xs text-emerald-300 hover:underline">
            Open
          </button>
        </div>
        {drives.length === 0 ? (
          <p className="text-xs text-slate-400">Plug in an external drive and Aegis will recognise it.</p>
        ) : (
          <ul className="space-y-2">
            {drives.map((d) => {
              const online = d.status === "online";
              const isBackup = d.role === "backup";
              return (
                <li key={d.id} className="flex items-center justify-between gap-2 text-sm">
                  <span className="flex min-w-0 items-center gap-2">
                    <span className={`h-2 w-2 shrink-0 rounded-full ${online ? "bg-emerald-400" : "bg-slate-600"}`} aria-hidden="true" />
                    <span className="truncate">{driveName(d.label, d.mount_point)}</span>
                    <span className={`shrink-0 rounded px-1.5 py-0.5 text-[11px] ${ROLE_CHIP[d.role] ?? ROLE_CHIP.unassigned}`}>
                      {ROLE_LABEL[d.role] ?? d.role}
                    </span>
                  </span>
                  <span className="shrink-0 text-xs text-slate-400">
                    {online ? "plugged in" : `last seen ${formatDay(d.last_seen)}`}
                    {isBackup && coverage && (
                      <span className={unprotected > 0 ? "ml-2 text-amber-300" : "ml-2 text-emerald-300"}>
                        {unprotected > 0 ? `◐ ${formatBytes(unprotected)} not backed up` : "✓ backup current"}
                      </span>
                    )}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </section>

      <section aria-label="Needs you" className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <h2 className="mb-3 text-sm font-semibold">Needs you</h2>
        {needs.length === 0 ? (
          <p className="text-xs text-slate-400">Nothing right now. Duplicates, backups and new drives show up here.</p>
        ) : (
          <ul className="space-y-2">
            {needs.map((n) => {
              const Icon = n.icon;
              return (
                <li key={n.key} className="flex items-center justify-between gap-2 text-sm">
                  <span className="flex min-w-0 items-center gap-2">
                    <Icon className="h-4 w-4 shrink-0 text-amber-300" aria-hidden="true" />
                    <span className="truncate">{n.text}</span>
                  </span>
                  <button
                    type="button"
                    onClick={() => onOpen(n.tab)}
                    className="shrink-0 rounded-md border border-slate-700 px-2 py-0.5 text-xs hover:bg-slate-800"
                  >
                    {n.action}
                  </button>
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </div>
  );
}
