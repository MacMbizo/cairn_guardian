import { useState } from "react";
import { CopyCheck, DatabaseBackup, FolderTree, Files, Trash2 } from "lucide-react";
import { aegis, errorMessage, type DupeChoice, type DupeGroup } from "../bindings/client";
import { driveName, formatBytes, formatDay } from "../format";
import type { DrivesData } from "./useDrives";

const KIND: Record<DupeGroup["kind"], { label: string; icon: typeof Files }> = {
  folder: { label: "Identical folders", icon: FolderTree },
  partial_folder: { label: "Nearly identical folders", icon: FolderTree },
  files: { label: "Duplicate files", icon: Files },
};

function GroupCard({
  group,
  preview,
  onDone,
  onReviewBatches,
}: {
  group: DupeGroup;
  preview: boolean;
  onDone: () => Promise<void>;
  onReviewBatches: () => void;
}) {
  const [keeper, setKeeper] = useState(group.keeper_index);
  const [msg, setMsg] = useState<{ text: string; batch?: boolean; offerBackupFor?: number } | null>(null);
  const [busy, setBusy] = useState(false);
  const Kind = KIND[group.kind];
  const hasBackupDrive = group.locations.some((l) => l.is_backup);

  const answer = async (choice: DupeChoice) => {
    if (preview) {
      setMsg({ text: "Preview only: run the desktop app to answer." });
      return;
    }
    setBusy(true);
    try {
      const out = await aegis.dupes.decide(group.group_id, choice);
      if (out.batch_id) {
        setMsg({ text: "Ready for your approval. Nothing has moved yet.", batch: true });
      } else {
        setMsg({ text: "Got it. Aegis won't ask about these again." });
        await onDone();
      }
    } catch (e) {
      const text = errorMessage(e);
      const idx = choice.choice === "delete_this_copy" ? choice.location_index : undefined;
      setMsg({ text, offerBackupFor: text.includes("Make this the backup copy") ? idx : undefined });
    } finally {
      setBusy(false);
    }
  };

  return (
    <article className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
      <header className="mb-3 flex flex-wrap items-center justify-between gap-2">
        <h3 className="flex items-center gap-2 text-sm font-semibold">
          <Kind.icon className="h-4 w-4 text-emerald-400" aria-hidden="true" />
          {Kind.label}
          <span className="text-xs font-normal text-slate-400">
            {group.file_count.toLocaleString()} file{group.file_count === 1 ? "" : "s"} · {formatBytes(group.size_bytes)} each copy
          </span>
        </h3>
      </header>

      <div className={`grid gap-3 ${group.locations.length === 2 ? "md:grid-cols-2" : "md:grid-cols-3"}`}>
        {group.locations.map((loc, i) => {
          const kept = i === keeper;
          return (
            <div key={`${loc.volume_id}-${loc.path}`} className={`rounded-lg border p-3 ${kept ? "border-emerald-500/50 bg-emerald-950/20" : "border-slate-700 bg-slate-950/40"}`}>
              <label className="flex cursor-pointer items-start gap-2">
                <input type="radio" name={`keep-${group.group_id}`} checked={kept} onChange={() => setKeeper(i)} className="mt-1 accent-emerald-500" />
                <span className="min-w-0">
                  <span className="block text-sm font-medium">
                    {driveName(loc.volume_label, "PC")}
                    {loc.is_backup && <span className="ml-2 rounded bg-sky-500/15 px-1.5 py-0.5 text-[11px] text-sky-300">Backup drive</span>}
                    {kept && <span className="ml-2 text-[11px] text-emerald-300">keep this one</span>}
                  </span>
                  <span className="block break-all font-mono text-xs text-slate-300">{loc.path}</span>
                  {!loc.online && <span className="block text-xs text-amber-300">on {driveName(loc.volume_label, "a drive")} (unplugged, last seen {formatDay(loc.last_seen)})</span>}
                </span>
              </label>
              {!kept && (
                <div className="mt-3 flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void answer({ choice: "make_this_the_backup_copy", location_index: i, keeper_index: keeper })}
                    className="flex items-center gap-1 rounded-md border border-sky-700 px-2 py-1 text-xs text-sky-200 hover:bg-sky-950/40 disabled:opacity-40"
                  >
                    <DatabaseBackup className="h-3.5 w-3.5" aria-hidden="true" /> It's a backup
                  </button>
                  <button
                    type="button"
                    disabled={busy}
                    onClick={() => void answer({ choice: "delete_this_copy", location_index: i, keeper_index: keeper })}
                    className="flex items-center gap-1 rounded-md border border-rose-800 px-2 py-1 text-xs text-rose-200 hover:bg-rose-950/40 disabled:opacity-40"
                  >
                    <Trash2 className="h-3.5 w-3.5" aria-hidden="true" /> Delete this copy
                  </button>
                </div>
              )}
            </div>
          );
        })}
      </div>

      {group.differences.length > 0 && (
        <details className="mt-3 text-xs text-slate-300">
          <summary className="cursor-pointer text-slate-400">{group.differences.length} files are only on one side</summary>
          <ul className="mt-1 list-disc pl-5 font-mono">
            {group.differences.slice(0, 20).map((d) => (
              <li key={d}>{d}</li>
            ))}
            {group.differences.length > 20 && <li>…and {group.differences.length - 20} more</li>}
          </ul>
        </details>
      )}

      <footer className="mt-3 flex flex-wrap items-center gap-3">
        <button type="button" disabled={busy} onClick={() => void answer({ choice: "keep_both" })} className="rounded-md border border-slate-700 px-2.5 py-1 text-xs hover:bg-slate-800 disabled:opacity-40">
          Keep both
        </button>
        {!hasBackupDrive && <span className="text-xs text-slate-500">Deletes go to quarantine for 30 days, and Aegis always keeps at least two copies.</span>}
        {msg && (
          <span role="status" className="text-xs text-slate-200">
            {msg.text}{" "}
            {msg.batch && (
              <button type="button" onClick={onReviewBatches} className="text-emerald-300 hover:underline">
                Review and approve
              </button>
            )}
            {msg.offerBackupFor !== undefined && (
              <button
                type="button"
                disabled={busy}
                onClick={() => void answer({ choice: "make_this_the_backup_copy", location_index: msg.offerBackupFor!, keeper_index: keeper })}
                className="ml-1 rounded-md bg-sky-700 px-2 py-0.5 text-white hover:bg-sky-600"
              >
                Make this the backup copy
              </button>
            )}
          </span>
        )}
      </footer>
    </article>
  );
}

export function DupeReview({ data, onReviewBatches }: { data: DrivesData; onReviewBatches: () => void }) {
  const groups = data.dupes?.groups.filter((g) => g.needs_question) ?? [];
  const answered = (data.dupes?.groups.length ?? 0) - groups.length;
  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="flex items-center gap-2 text-sm font-semibold">
          <CopyCheck className="h-4 w-4 text-emerald-400" aria-hidden="true" />
          {groups.length === 0 ? "No duplicate questions" : `${groups.length} duplicate question${groups.length === 1 ? "" : "s"}`}
          {data.dupes && data.dupes.reclaimable_bytes > 0 && (
            <span className="text-xs font-normal text-slate-400">up to {formatBytes(data.dupes.reclaimable_bytes)} to reclaim</span>
          )}
        </h2>
        {answered > 0 && <span className="text-xs text-slate-500">{answered} already answered or safely backed up</span>}
      </div>
      {groups.length === 0 && (
        <p className="text-xs text-slate-400">Catalog your drives and Aegis will group exact copies by folder, so you answer once per folder, not once per file.</p>
      )}
      {groups.map((g) => (
        <GroupCard key={g.group_id} group={g} preview={data.preview} onDone={data.reload} onReviewBatches={onReviewBatches} />
      ))}
    </div>
  );
}
