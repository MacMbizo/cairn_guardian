import { useEffect, useState } from "react";
import { Pause, ShieldAlert, ShieldX, Usb, X } from "lucide-react";
import { aegis, errorMessage, onAegisEvent, type RefreshReport } from "../bindings/client";
import { driveName, formatBytes, formatDay } from "../format";
import type { DrivesTab } from "./HomeTiles";

type Banner =
  | { kind: "arrived"; volumeId: string; label: string; lastSeen: number; known: boolean; report?: RefreshReport }
  | { kind: "cataloguing"; volumeId: string; label: string; files: number }
  | { kind: "topup"; done: number; total: number }
  | { kind: "topup_done"; text: string }
  | { kind: "halted"; text: string }
  | { kind: "tripped"; folder: string; name: string; changed: number; scrambled: number }
  | { kind: "error"; text: string };

/** One-line strip under the header: plugged-in drives, backup progress and results (spec §7.4). */
export function PlugBanner({ onOpen, preview = false }: { onOpen: (tab: DrivesTab) => void; preview?: boolean }) {
  const [banner, setBanner] = useState<Banner | null>(null);

  // Browser preview: `?tripped=1` shows the ransomware banner with sample numbers.
  useEffect(() => {
    if (preview && new URLSearchParams(window.location.search).has("tripped")) {
      setBanner({ kind: "tripped", folder: "C:\\Users\\you\\Documents", name: "Documents", changed: 412, scrambled: 18 });
    }
  }, [preview]);

  // The tripwire may have fired while the window was closed: a locked folder always shows the
  // banner, so "It was me" is reachable from any screen.
  useEffect(() => {
    if (preview) return;
    void aegis.engine
      .tripwireLocks()
      .then((locks) => {
        const folder = locks[0];
        if (folder)
          setBanner((b) => b ?? { kind: "tripped", folder, name: folder.split(/[\\/]/).pop() ?? folder, changed: 0, scrambled: 0 });
      })
      .catch(() => {});
  }, [preview]);

  useEffect(() => {
    const unlisten = onAegisEvent((ev) => {
      switch (ev.event) {
        case "DriveArrived":
          setBanner({
            kind: "arrived",
            volumeId: ev.payload.volume_id,
            label: driveName(ev.payload.label, "Drive"),
            lastSeen: ev.payload.last_seen,
            known: ev.payload.known,
          });
          break;
        case "DriveRefreshed":
          setBanner((b) =>
            b && b.kind === "arrived" && b.volumeId === ev.payload.report.volume_id ? { ...b, report: ev.payload.report } : b
          );
          break;
        case "DriveCatalogProgress":
          setBanner((b) =>
            b && b.kind === "cataloguing" && b.volumeId === ev.payload.volume_id ? { ...b, files: ev.payload.files_seen } : b
          );
          break;
        case "DriveCatalogued":
          setBanner((b) =>
            b && b.kind === "cataloguing" && b.volumeId === ev.payload.volume_id
              ? { kind: "topup_done", text: `Catalogued ${b.label}: ${ev.payload.files.toLocaleString()} files.` }
              : b
          );
          break;
        case "TopUpProgress":
          setBanner({ kind: "topup", done: ev.payload.bytes_done, total: ev.payload.bytes_total });
          break;
        case "TopUpFinished": {
          const r = ev.payload.result;
          const files = r.added + r.replaced;
          setBanner({
            kind: "topup_done",
            text: r.completed
              ? files === 0 && r.retired === 0
                ? "Backup up to date."
                : `Backup up to date: ${files} file${files === 1 ? "" : "s"} added, ${formatBytes(r.bytes_copied)}.`
              : `Backup paused after ${formatBytes(r.bytes_copied)}. It carries on next time.`,
          });
          break;
        }
        case "GuardHalted": {
          const h = ev.payload.halts[0];
          const name = h.source_folder.split(/[\\/]/).pop();
          setBanner({
            kind: "halted",
            text: `An unusual number of files changed in ${name} (${h.changed} of ${h.total}). Those weren't backed up yet.`,
          });
          break;
        }
        case "Tripped": {
          const t = ev.payload.trip;
          setBanner({
            kind: "tripped",
            folder: t.root,
            name: t.root.split(/[\\/]/).pop() ?? t.root,
            changed: t.changed,
            scrambled: t.suspect_encrypted,
          });
          break;
        }
        case "JobFailed":
          setBanner({ kind: "error", text: ev.payload.message });
          break;
        default:
          break;
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  if (!banner) return null;
  const close = () => setBanner(null);
  const base = "flex items-center justify-between gap-3 border-b px-6 py-2 text-xs";

  if (banner.kind === "arrived") {
    const r = banner.report;
    const changes = r ? r.created + r.modified + r.renamed + r.deleted : 0;
    return (
      <div role="status" className={`${base} border-sky-500/30 bg-sky-950/30 text-sky-100`}>
        <span className="flex items-center gap-2">
          <Usb className="h-4 w-4 text-sky-300" aria-hidden="true" />
          {banner.known ? (
            <span>
              <strong>{banner.label}</strong>, last seen {formatDay(banner.lastSeen)}
              {r
                ? ` · ${r.created.toLocaleString()} new files · ${r.folders_changed} folders changed · ${r.new_duplicates} new duplicates`
                : " · checking what changed…"}
            </span>
          ) : (
            <span>
              New drive <strong>{banner.label}</strong>. A quick catalog lets Cairn find duplicates and remember what's on it.
            </span>
          )}
        </span>
        <span className="flex shrink-0 gap-2">
          {banner.known ? (
            <button
              type="button"
              onClick={() => onOpen(changes > 0 || (r?.new_duplicates ?? 0) > 0 ? "dupes" : "drives")}
              className="rounded-md bg-sky-600 px-2.5 py-1 font-semibold text-white hover:bg-sky-500"
            >
              Review
            </button>
          ) : (
            <button
              type="button"
              onClick={() => {
                // Switch to the progress line at once, so there is nothing left to click twice.
                setBanner({ kind: "cataloguing", volumeId: banner.volumeId, label: banner.label, files: 0 });
                void aegis.drives.catalog(banner.volumeId).catch((e) => setBanner({ kind: "error", text: errorMessage(e) }));
              }}
              className="rounded-md bg-sky-600 px-2.5 py-1 font-semibold text-white hover:bg-sky-500"
            >
              Catalog it
            </button>
          )}
          <button type="button" onClick={close} className="rounded-md border border-sky-700 px-2.5 py-1 hover:bg-sky-900/50">
            Later
          </button>
        </span>
      </div>
    );
  }

  if (banner.kind === "tripped") {
    return (
      <div role="alert" className={`${base} border-rose-500/40 bg-rose-950/40 text-rose-100`}>
        <span className="flex items-center gap-2">
          <ShieldX className="h-4 w-4 shrink-0 text-rose-300" aria-hidden="true" />
          {banner.changed > 0 ? (
            <span>
              <strong>Lots of files in {banner.name} just changed at once</strong> ({banner.changed.toLocaleString()} in a minute
              {banner.scrambled > 0 ? `, ${banner.scrambled} look scrambled` : ""}). Backup of this folder is paused so nothing bad gets copied.
            </span>
          ) : (
            <span>
              <strong>Backup of {banner.name} is paused</strong> because lots of files there changed at once. Check them, then tell Cairn if it was you.
            </span>
          )}
        </span>
        <span className="flex shrink-0 gap-2">
          <button
            type="button"
            onClick={() =>
              preview
                ? close()
                : void aegis.engine
                    .tripwireWasMe(banner.folder)
                    .then(() => setBanner({ kind: "topup_done", text: `Thanks. ${banner.name} is backed up as normal again.` }))
                    .catch((e) => setBanner({ kind: "error", text: errorMessage(e) }))
            }
            className="rounded-md border border-rose-700 px-2.5 py-1 hover:bg-rose-900/50"
          >
            It was me
          </button>
          <button type="button" onClick={() => onOpen("drives")} className="rounded-md bg-rose-600 px-2.5 py-1 font-semibold text-white hover:bg-rose-500">
            Show me
          </button>
        </span>
      </div>
    );
  }

  if (banner.kind === "cataloguing") {
    return (
      <div role="status" className={`${base} border-sky-500/30 bg-sky-950/30 text-sky-100`}>
        <span className="flex items-center gap-2">
          <Usb className="h-4 w-4 text-sky-300" aria-hidden="true" />
          Cataloguing <strong>{banner.label}</strong>… {banner.files > 0 ? `${banner.files.toLocaleString()} files so far` : "starting"}. You can keep working; leave the drive plugged in until it finishes.
        </span>
        <button type="button" onClick={close} aria-label="Hide" className="rounded-md p-1 hover:bg-white/10">
          <X className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </div>
    );
  }

  if (banner.kind === "topup") {
    const pct = banner.total > 0 ? Math.min(100, (banner.done / banner.total) * 100) : 0;
    return (
      <div role="status" className={`${base} border-emerald-500/30 bg-emerald-950/30 text-emerald-100`}>
        <span className="flex flex-1 items-center gap-3">
          <span>
            Backing up… {formatBytes(banner.done)} of {formatBytes(banner.total)}
          </span>
          <span className="h-1.5 max-w-64 flex-1 overflow-hidden rounded bg-emerald-900/60">
            <span className="block h-full bg-emerald-400" style={{ width: `${pct}%` }} />
          </span>
        </span>
        <button
          type="button"
          onClick={() => void aegis.backup.pause()}
          className="flex items-center gap-1 rounded-md border border-emerald-700 px-2.5 py-1 hover:bg-emerald-900/50"
        >
          <Pause className="h-3.5 w-3.5" aria-hidden="true" /> Pause
        </button>
      </div>
    );
  }

  const tone =
    banner.kind === "topup_done"
      ? "border-emerald-500/30 bg-emerald-950/30 text-emerald-100"
      : "border-amber-500/30 bg-amber-950/30 text-amber-100";
  return (
    <div role="status" className={`${base} ${tone}`}>
      <span className="flex items-center gap-2">
        {banner.kind !== "topup_done" && <ShieldAlert className="h-4 w-4 text-amber-300" aria-hidden="true" />}
        {banner.text}
      </span>
      <span className="flex shrink-0 gap-2">
        {banner.kind === "halted" && (
          <button type="button" onClick={() => onOpen("drives")} className="rounded-md bg-amber-600 px-2.5 py-1 font-semibold text-white hover:bg-amber-500">
            Check
          </button>
        )}
        <button type="button" onClick={close} aria-label="Dismiss" className="rounded-md p-1 hover:bg-white/10">
          <X className="h-3.5 w-3.5" aria-hidden="true" />
        </button>
      </span>
    </div>
  );
}
