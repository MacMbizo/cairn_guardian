import { useEffect, useState } from "react";
import { Keyboard, MoonStar, Power, Undo2 } from "lucide-react";
import { aegis, errorMessage } from "../bindings/client";
import { DAY_NAMES } from "./ThisWeek";
import type { EngineData } from "./useEngine";

const card = "rounded-xl border border-slate-800 bg-slate-900/50 p-4";
const btn = "rounded-md border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40";
const KIND_LABEL: Record<string, string> = {
  only_copy: "Files that exist only on this PC",
  quarantine_expiry: "Quarantine about to empty",
  space: "Running out of space",
  janitor: "Downloads tidy-up",
};

function shortcutText(accel: string): string {
  return accel.replace("CommandOrControl", "Ctrl").replaceAll("+", " + ");
}

/** Settings for the always-on engine (spec §10). */
export function EngineSettings({ data }: { data: EngineData }) {
  const { status, preview, reload } = data;
  const [msg, setMsg] = useState<string | null>(null);
  const [shortcut, setShortcut] = useState("");

  useEffect(() => {
    if (status) setShortcut(status.shortcut);
  }, [status]);

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

  if (!status) return <p className="text-xs text-slate-400">Loading…</p>;
  const paused = status.paused_until != null;

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      {msg && (
        <p role="status" className="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-200">
          {msg}
        </p>
      )}

      <section aria-label="Running in the background" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Power className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Running in the background
        </h2>
        <label className="flex items-center justify-between gap-3 text-sm">
          <span>
            Start with Windows
            <span className="block text-xs text-slate-400">Aegis sits in the tray and keeps watching. Closing the window doesn't stop it.</span>
          </span>
          <input
            type="checkbox"
            checked={status.autostart}
            onChange={(e) => void run(() => aegis.engine.settingsSet({ autostart: e.target.checked }))}
            className="h-4 w-4 accent-emerald-500"
          />
        </label>
      </section>

      <section aria-label="Weekly note" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <MoonStar className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Weekly note
        </h2>
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span>Arrives on</span>
          <select
            aria-label="Note day"
            value={status.note_day}
            onChange={(e) => void run(() => aegis.engine.settingsSet({ noteDay: Number(e.target.value) }))}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-xs"
          >
            {DAY_NAMES.map((d, i) => (
              <option key={d} value={i}>
                {d}
              </option>
            ))}
          </select>
          <span>at</span>
          <select
            aria-label="Note hour"
            value={status.note_hour}
            onChange={(e) => void run(() => aegis.engine.settingsSet({ noteHour: Number(e.target.value) }))}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-xs"
          >
            {Array.from({ length: 24 }, (_, h) => (
              <option key={h} value={h}>
                {String(h).padStart(2, "0")}:00
              </option>
            ))}
          </select>
        </div>
      </section>

      <section aria-label="Search shortcut" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Keyboard className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Find anything, anywhere
        </h2>
        <p className="mb-2 text-xs text-slate-400">
          Press <kbd className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-200">{shortcutText(status.shortcut)}</kbd> from any app to search every drive, even unplugged ones.
        </p>
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void run(() => aegis.engine.settingsSet({ shortcut: shortcut.trim() }));
          }}
        >
          <input
            aria-label="Shortcut"
            value={shortcut}
            onChange={(e) => setShortcut(e.target.value)}
            className="w-72 rounded border border-slate-700 bg-slate-950 px-2 py-1 font-mono text-xs"
          />
          <button type="submit" className={btn} disabled={!shortcut.trim() || shortcut === status.shortcut}>
            Use this shortcut
          </button>
        </form>
        {!status.shortcut_ok && <p className="mt-2 text-xs text-amber-300">That shortcut is taken; pick another.</p>}
      </section>

      <section aria-label="Pause" className={card}>
        <h2 className="mb-1 text-sm font-semibold">Pause</h2>
        <p className="mb-3 text-xs text-slate-400">
          Pausing holds tidy-ups and gentle reminders. Watching for ransomware and keeping the index current never pause.
        </p>
        <div className="flex flex-wrap gap-2">
          {paused ? (
            <button type="button" className={btn} onClick={() => void run(() => aegis.engine.pause("resume"))}>
              Resume now
            </button>
          ) : (
            <>
              <button type="button" className={btn} onClick={() => void run(() => aegis.engine.pause("hour"))}>
                Pause for 1 hour
              </button>
              <button type="button" className={btn} onClick={() => void run(() => aegis.engine.pause("tomorrow"))}>
                Pause until tomorrow
              </button>
            </>
          )}
        </div>
      </section>

      <section aria-label="Reminders moved to your note" className={card}>
        <h2 className="mb-1 text-sm font-semibold">Reminders moved to your note</h2>
        <p className="mb-3 text-xs text-slate-400">When you wave away the same kind of reminder three times, Aegis stops popping it up and puts it in your weekly note instead.</p>
        {status.demoted.length === 0 ? (
          <p className="text-xs text-slate-400">None.</p>
        ) : (
          <ul className="divide-y divide-slate-800">
            {status.demoted.map((k) => (
              <li key={k} className="flex items-center justify-between gap-2 py-2 text-sm">
                <span>{KIND_LABEL[k] ?? k}</span>
                <button type="button" className={btn} onClick={() => void run(() => aegis.engine.undemote(k))}>
                  <Undo2 className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Remind me again
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
