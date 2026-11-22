import { useEffect, useState } from "react";
import { Activity, BellRing, CheckCheck, Download, HardDrive, ShieldCheck } from "lucide-react";
import { aegis, errorMessage, type FindingAction, type Reminder, type Screen } from "../bindings/client";
import type { EngineData } from "./useEngine";

export const DAY_NAMES = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

const card = "rounded-xl border border-slate-800 bg-slate-900/50 p-4";

export function screenFor(action: FindingAction): Screen {
  if (action.type === "open") return action.screen;
  if (action.type === "batch") return "simulator";
  return "week";
}

function Section({ icon: Icon, title, children }: { icon: typeof BellRing; title: string; children: React.ReactNode }) {
  return (
    <section aria-label={title} className={card}>
      <h2 className="mb-2 flex items-center gap-2 text-sm font-semibold">
        <Icon className="h-4 w-4 text-emerald-400" aria-hidden="true" /> {title}
      </h2>
      {children}
    </section>
  );
}

/** The weekly note (spec §9): one calm page, sections with nothing to say are left out. */
/** The note builder's wording for automatic filing (`engine::note::build`). */
const AUTO_FILED = /^Filed .+ by itself$/;

export function ThisWeek({
  data,
  onOpen,
  onOpenRecentFiling,
}: {
  data: EngineData;
  onOpen: (s: Screen) => void;
  onOpenRecentFiling?: () => void;
}) {
  const { note, status, preview, reload } = data;
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    if (note && !note.read && !preview) void aegis.engine.noteMarkRead(note.week).catch(() => {});
  }, [note, preview]);

  const run = async (fn: () => Promise<string | void>) => {
    if (preview) {
      setMsg("Preview only: run the desktop app to make changes.");
      return;
    }
    try {
      const out = await fn();
      if (out) setMsg(out);
      await reload();
    } catch (e) {
      setMsg(errorMessage(e));
    }
  };

  const showMe = (r: Reminder) =>
    void run(async () => {
      // A tripwire alert stays until its folder is unlocked ("It was me" on the banner).
      if (r.kind !== "tripwire") await aegis.engine.act(r.id);
      onOpen(screenFor(r.action));
    });
  const notNow = (r: Reminder) =>
    void run(async () => {
      const demoted = await aegis.engine.dismiss(r.id);
      return demoted ? "Got it. Reminders like this will wait for your weekly note from now on." : undefined;
    });

  if (!note) {
    const day = DAY_NAMES[status?.note_day ?? 6];
    return (
      <div className={card}>
        <h2 className="mb-1 text-sm font-semibold">This week</h2>
        <p className="text-sm text-slate-300">Your first note arrives on {day} evening.</p>
        <p className="mt-1 text-xs text-slate-400">Aegis keeps watching in the background and saves the quiet things for it.</p>
      </div>
    );
  }

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      <header>
        <p className="text-xs uppercase tracking-wide text-slate-400">
          Week ending {new Date(`${note.week}T12:00:00`).toLocaleDateString([], { weekday: "long", day: "numeric", month: "long" })}
        </p>
        <h1 className="mt-1 text-xl font-semibold text-slate-50">{note.headline}</h1>
      </header>

      {msg && (
        <p role="status" className="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-200">
          {msg}
        </p>
      )}

      {note.needs_you.length > 0 && (
        <Section icon={BellRing} title="Needs you">
          <ul className="divide-y divide-slate-800">
            {note.needs_you.map((r) => (
              <li key={r.id} className="flex flex-wrap items-center justify-between gap-2 py-2">
                <span className="min-w-0">
                  <span className={`block text-sm ${r.level === "now" ? "text-rose-200" : "text-slate-100"}`}>{r.title}</span>
                  <span className="block text-xs text-slate-400">{r.detail}</span>
                </span>
                <span className="flex shrink-0 gap-2">
                  <button type="button" onClick={() => showMe(r)} className="rounded-md bg-emerald-600 px-2.5 py-1 text-xs font-semibold text-white hover:bg-emerald-500">
                    Show me
                  </button>
                  <button type="button" onClick={() => notNow(r)} className="rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800">
                    Not now
                  </button>
                </span>
              </li>
            ))}
          </ul>
        </Section>
      )}

      {note.did.length > 0 && (
        <Section icon={CheckCheck} title="What Aegis did by itself">
          <ul className="list-inside list-disc space-y-0.5 text-sm text-slate-300">
            {note.did.map((d) => (
              <li key={d}>
                {AUTO_FILED.test(d) && onOpenRecentFiling ? (
                  <button type="button" onClick={onOpenRecentFiling} className="text-emerald-300 hover:underline">
                    {d} — see or undo
                  </button>
                ) : (
                  d
                )}
              </li>
            ))}
          </ul>
        </Section>
      )}

      {note.safety && (
        <Section icon={ShieldCheck} title="Safety">
          <p className="text-sm text-slate-300">{note.safety}</p>
        </Section>
      )}

      {note.space.length > 0 && (
        <Section icon={HardDrive} title="Space">
          {note.space.map((s) => (
            <p key={s} className="text-sm text-slate-300">
              {s}
            </p>
          ))}
        </Section>
      )}

      {note.tidy && (
        <Section icon={Download} title="Tidy-up found">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <p className="text-sm text-slate-300">{note.tidy}</p>
            <button type="button" onClick={() => onOpen("simulator")} className="rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800">
              Review
            </button>
          </div>
        </Section>
      )}

      {note.about_aegis.length > 0 && (
        <Section icon={Activity} title="About Aegis">
          <ul className="space-y-0.5 text-xs text-slate-400">
            {note.about_aegis.map((a) => (
              <li key={a}>{a}</li>
            ))}
          </ul>
        </Section>
      )}
    </div>
  );
}
