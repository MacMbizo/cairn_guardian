import { ChevronRight } from "lucide-react";
import type { HealthBreakdown, QuarantineSpaceSummary } from "../bindings";

type Tab = "care" | "drives" | "dupes" | "filing" | "rescue";

function formatBytes(n: number): string {
  if (n <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  return `${(n / 1024 ** i).toFixed(i >= 3 ? 1 : 0)} ${units[i]}`;
}

/**
 * D1 storage clarity bar: files in use, items waiting in quarantine (still taking space, still
 * restorable) and free space, across every catalogued drive, as one 100% stacked bar.
 */
export function StorageClarity({
  summaries,
  onOpen,
}: {
  summaries: QuarantineSpaceSummary[];
  onOpen: (tab: Tab) => void;
}) {
  const total = summaries.reduce((a, s) => a + s.volume_total_bytes, 0);
  const free = summaries.reduce((a, s) => a + s.volume_free_bytes, 0);
  const staged = summaries.reduce((a, s) => a + s.staged_bytes, 0);
  const freed = summaries.reduce((a, s) => a + s.freed_bytes, 0);
  const inUse = Math.max(0, total - free - staged);
  const pct = (n: number) => (total > 0 ? (n / total) * 100 : 0);
  const quotaWarning = summaries.some((s) => s.quota_warning);

  return (
    <section aria-labelledby="storage-title" className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 id="storage-title" className="text-sm font-semibold text-slate-100">
          Storage &amp; quarantine
        </h2>
        {quotaWarning && (
          <span className="rounded-full border border-amber-700/60 bg-amber-950/40 px-2 py-0.5 text-[11px] text-amber-200">
            Quarantine is over 10% of a drive
          </span>
        )}
      </div>

      {total > 0 ? (
        <>
          <div
            className="mt-3 flex h-3 w-full overflow-hidden rounded-full bg-slate-800"
            role="img"
            aria-label={`${formatBytes(inUse)} in use, ${formatBytes(staged)} waiting in quarantine, ${formatBytes(free)} free`}
          >
            <div className="h-full bg-slate-500" style={{ width: `${pct(inUse)}%` }} />
            <div
              className="h-full bg-amber-500"
              style={{
                width: `${pct(staged)}%`,
                backgroundImage:
                  "repeating-linear-gradient(135deg, rgba(15,23,42,.45) 0 3px, transparent 3px 7px)",
              }}
            />
            <div className="h-full bg-emerald-500" style={{ width: `${pct(free)}%` }} />
          </div>
          <ul className="mt-2 flex flex-wrap gap-x-4 gap-y-1 text-xs text-slate-300">
            <li className="flex items-center gap-1.5">
              <span className="h-2.5 w-2.5 rounded-sm bg-slate-500" aria-hidden="true" /> In use {formatBytes(inUse)}
            </li>
            <li className="flex items-center gap-1.5">
              <span className="h-2.5 w-2.5 rounded-sm bg-amber-500" aria-hidden="true" /> In quarantine {formatBytes(staged)} (restorable for 30 days)
            </li>
            <li className="flex items-center gap-1.5">
              <span className="h-2.5 w-2.5 rounded-sm bg-emerald-500" aria-hidden="true" /> Free {formatBytes(free)}
            </li>
          </ul>
        </>
      ) : (
        <p className="mt-2 text-xs text-slate-400">Drive sizes appear once Cairn has catalogued a folder.</p>
      )}

      <div className="mt-4 flex flex-wrap items-end justify-between gap-3 border-t border-slate-800 pt-3">
        <div className="flex gap-6">
          <div>
            <div className="font-mono text-xl text-amber-300">{formatBytes(staged)}</div>
            <div className="text-[11px] text-slate-400">Waiting in quarantine</div>
          </div>
          <div>
            <div className="font-mono text-xl text-emerald-400">{formatBytes(freed)}</div>
            <div className="text-[11px] text-slate-400">Actually freed on disk</div>
          </div>
        </div>
        <button
          type="button"
          onClick={() => onOpen("rescue")}
          className="rounded-md border border-slate-700 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-400"
        >
          Review quarantine
        </button>
      </div>
    </section>
  );
}

const DIMENSION_TAB: Record<string, Tab> = {
  storage: "care",
  backup: "drives",
  duplication: "dupes",
  caches: "care",
  organization: "filing",
};

function barColour(score: number): string {
  if (score >= 85) return "bg-emerald-500";
  if (score >= 60) return "bg-amber-500";
  return "bg-rose-500";
}

/** D1 health bars: the overall score and each of the five dimensions, each opening its screen. */
export function HealthBars({ health, onOpen }: { health: HealthBreakdown | null; onOpen: (tab: Tab) => void }) {
  return (
    <section aria-labelledby="health-title" className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
      <div className="flex items-baseline justify-between">
        <h2 id="health-title" className="text-sm font-semibold text-slate-100">
          Workspace health
        </h2>
        <span className="font-mono text-lg text-emerald-400">
          {health ? Math.round(health.overall_score) : "—"}
          <span className="text-xs text-slate-500"> / 100</span>
        </span>
      </div>
      {health ? (
        <ul className="mt-3 space-y-1">
          {health.dimensions.map((d) => {
            const target = DIMENSION_TAB[d.id] ?? "care";
            return (
              <li key={d.id}>
                <button
                  type="button"
                  onClick={() => onOpen(target)}
                  className="group grid w-full grid-cols-[8.5rem_1fr_2rem_auto] items-center gap-3 rounded-md px-1.5 py-1 text-left text-xs hover:bg-slate-800/60 focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-400"
                >
                  <span className="truncate text-slate-300">{d.label}</span>
                  <span className="h-1.5 overflow-hidden rounded-full bg-slate-800" aria-hidden="true">
                    <span className={`block h-full ${barColour(d.score)}`} style={{ width: `${Math.max(2, d.score)}%` }} />
                  </span>
                  <span className="text-right font-mono text-slate-200">{Math.round(d.score)}</span>
                  <span className="flex items-center gap-0.5 text-slate-500 group-hover:text-slate-300">
                    {d.offenders_count > 0 ? `${d.offenders_count} to look at` : "fine"}
                    <ChevronRight className="h-3.5 w-3.5" aria-hidden="true" />
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="mt-2 text-xs text-slate-400">The score appears after the first catalog.</p>
      )}
    </section>
  );
}
