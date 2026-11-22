import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { ArrowLeft, Copy, GitBranch, HardDrive, History, House, Layers, Loader2, RefreshCw, SearchCheck } from "lucide-react";
import { aegis, errorMessage, type Answer, type Collection, type Location, type Relationship } from "../bindings/client";
import { fixtureProjects } from "./fixtures/projects";

const card = "rounded-xl border border-slate-800 bg-slate-900/50";
const btn = "rounded-md border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40";
const primary = "rounded-md bg-emerald-700 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-600 disabled:opacity-40";

const SURE = 0.8;
const SEP = /[\\/]/;

const nameOf = (p: string) => p.split(SEP).filter(Boolean).pop() ?? p;
const parentOf = (p: string) => p.split(SEP).slice(0, -1).join("\\");

/** Shortens a path to its last few folders so rows stay readable. */
function shortPath(p: string, keep = 4): string {
  const parts = p.split(SEP).filter(Boolean);
  return parts.length <= keep ? parts.join("\\") : `…\\${parts.slice(-keep).join("\\")}`;
}

const KIND_TEXT: Record<string, string> = {
  software: "Software",
  design: "Design",
  writing: "Writing",
  research: "Research",
  other: "Other",
};

/** How a copy relates to the main copy, in plain words, with its icon and colour. */
function relation(l: Location): { label: string; detail: string; icon: ReactNode; tone: string } {
  const shared = l.shared_files.toLocaleString();
  const rel: Record<Relationship, { label: string; detail: string; icon: ReactNode; tone: string }> = {
    home: { label: "Main copy", detail: "Newest and most complete", icon: <House className="h-4 w-4" aria-hidden="true" />, tone: "text-emerald-300" },
    identical: { label: "Identical copy", detail: "Holds the same files as the main copy", icon: <Copy className="h-4 w-4" aria-hidden="true" />, tone: "text-slate-300" },
    older_subset: {
      label: "Older copy",
      detail: "Everything here is also in the main copy",
      icon: <Copy className="h-4 w-4" aria-hidden="true" />,
      tone: "text-slate-300",
    },
    diverged: {
      label: "Changed on both sides",
      detail: `${shared} files in common; each copy also has files the other lacks`,
      icon: <GitBranch className="h-4 w-4" aria-hidden="true" />,
      tone: "text-amber-300",
    },
    different_material: {
      label: "Different material",
      detail: "Same project, but little in common with the main copy",
      icon: <Layers className="h-4 w-4" aria-hidden="true" />,
      tone: "text-sky-300",
    },
    version: { label: "Another version", detail: `${shared} files in common with the main copy`, icon: <History className="h-4 w-4" aria-hidden="true" />, tone: "text-sky-300" },
    unrelated: { label: "Not the same project", detail: "", icon: <Layers className="h-4 w-4" aria-hidden="true" />, tone: "text-slate-400" },
    unknown: {
      label: "Possibly the same project",
      detail: "Only the name matches so far",
      icon: <SearchCheck className="h-4 w-4" aria-hidden="true" />,
      tone: "text-amber-300",
    },
  };
  return rel[l.relationship];
}

/** Evidence the card shows under "Why Aegis thinks so" (links to other copies are shown in the table). */
const whyOf = (c: Collection) => c.evidence.filter((e) => !e.startsWith("also at ")).slice(0, 4);

function Chip({ children, tone }: { children: ReactNode; tone: string }) {
  return <span className={`rounded px-1.5 py-0.5 text-[11px] font-medium ${tone}`}>{children}</span>;
}

function SureChip({ c }: { c: Collection }) {
  if (c.state === "left_as_is") return <Chip tone="bg-slate-800 text-slate-300">left as is</Chip>;
  if (c.state === "confirmed" || c.confidence >= SURE) return <Chip tone="bg-emerald-500/15 text-emerald-300">sure</Chip>;
  return <Chip tone="bg-amber-500/15 text-amber-300">Is this a project?</Chip>;
}

interface Answering {
  busy: boolean;
  send: (c: Collection, a: Answer, locationId?: string) => Promise<void>;
}

function PlaceRow({ c, l, ans }: { c: Collection; l: Location; ans: Answering }) {
  const r = relation(l);
  const home = l.relationship === "home";
  return (
    <li
      className={
        home
          ? "flex items-center gap-3 rounded-lg border border-emerald-800 bg-emerald-950/60 px-3 py-2.5"
          : "flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 px-1 py-2.5 last:border-b-0 sm:flex-nowrap"
      }
    >
      <span className="flex min-w-0 flex-1 items-center gap-2.5">
        <span className={`shrink-0 ${r.tone}`}>{r.icon}</span>
        <span className="min-w-0">
          <span className="block truncate text-xs font-medium text-slate-100" title={l.root_path}>
            {shortPath(l.root_path)}
          </span>
          <span className={`block text-xs ${r.tone}`}>
            {r.label}
            {r.detail && <span className="text-slate-400"> · {r.detail}</span>}
            {home && <span className="text-slate-400"> · {l.files.toLocaleString()} files</span>}
          </span>
          {!l.online && (
            <span className="mt-0.5 flex items-center gap-1 text-[11px] text-slate-400">
              <HardDrive className="h-3 w-3" aria-hidden="true" /> On a drive that is unplugged
            </span>
          )}
        </span>
      </span>
      {!home && c.state !== "left_as_is" && (
        <button type="button" className={`${btn} shrink-0`} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "not_same_as" }, l.id)}>
          Not this project
        </button>
      )}
    </li>
  );
}

function ProjectCard({ c, ans }: { c: Collection; ans: Answering }) {
  const [renaming, setRenaming] = useState(false);
  const [name, setName] = useState(c.name);
  const home = c.locations.find((l) => l.relationship === "home") ?? c.locations[0];
  const others = c.locations.filter((l) => l !== home);
  return (
    <section aria-label={c.name} className={`${card} grid grid-cols-1 md:grid-cols-[240px_minmax(0,1fr)]`}>
      <div className="flex flex-col gap-3 border-b border-slate-800 p-4 md:border-b-0 md:border-r">
        <div className="space-y-1.5">
          {renaming ? (
            <form
              className="flex gap-1.5"
              onSubmit={(e) => {
                e.preventDefault();
                void ans.send(c, { kind: "rename", name }).then(() => setRenaming(false));
              }}
            >
              <label className="sr-only" htmlFor={`name-${c.id}`}>
                Project name
              </label>
              <input
                id={`name-${c.id}`}
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="min-w-0 flex-1 rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-sm text-slate-100"
              />
              <button type="submit" className={primary} disabled={ans.busy || !name.trim()}>
                Save
              </button>
            </form>
          ) : (
            <h2 className="text-base font-semibold text-slate-100">{c.name}</h2>
          )}
          <div className="flex flex-wrap gap-1.5">
            <Chip tone="bg-slate-800 text-slate-300">{KIND_TEXT[c.project_kind ?? "other"]}</Chip>
            <SureChip c={c} />
            <Chip tone="bg-slate-800 text-slate-300">
              {c.locations.length} {c.locations.length === 1 ? "place" : "places"}
            </Chip>
          </div>
        </div>
        <div>
          <p className="text-xs font-semibold text-slate-400">Why Aegis thinks so</p>
          <ul className="mt-1 list-disc space-y-0.5 pl-4 text-xs text-slate-300">
            {whyOf(c).map((e) => (
              <li key={e}>{e.charAt(0).toUpperCase() + e.slice(1)}</li>
            ))}
          </ul>
        </div>
        <div className="mt-auto flex flex-col gap-2">
          {c.state !== "left_as_is" && (
            <button type="button" className={primary} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "leave_as_is" })}>
              Leave as is
            </button>
          )}
          <div className="flex gap-2">
            <button type="button" className={`${btn} flex-1`} disabled={ans.busy} onClick={() => setRenaming((r) => !r)}>
              {renaming ? "Cancel" : "Rename"}
            </button>
            <button type="button" className={`${btn} flex-1`} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "is_project", yes: false })}>
              Not a project
            </button>
          </div>
        </div>
      </div>
      <div className="flex flex-col gap-2 p-4">
        <p className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">Where it lives</p>
        <ul className="flex flex-col gap-1">
          {home && <PlaceRow c={c} l={home} ans={ans} />}
          {others.map((l) => (
            <PlaceRow key={l.id} c={c} l={l} ans={ans} />
          ))}
        </ul>
        {others.length > 0 && <p className="mt-auto pt-2 text-xs text-slate-400">Plans to bring these together come next.</p>}
      </div>
    </section>
  );
}

function ConfirmList({ items, ans }: { items: Collection[]; ans: Answering }) {
  if (items.length === 0) return null;
  return (
    <section aria-label="Folders to confirm" className={`${card} p-4`}>
      <h2 className="text-xs font-semibold text-amber-300">
        {items.length} {items.length === 1 ? "folder" : "folders"} to confirm
      </h2>
      <p className="mt-0.5 text-xs text-slate-400">Filing leaves each one whole until you answer.</p>
      <ul className="mt-2">
        {items.map((c) => {
          const l = c.locations[0];
          return (
            <li key={c.id} className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 py-2 last:border-b-0">
              <span className="min-w-0 text-xs">
                <span className="font-medium text-slate-100">{c.name}</span>
                <span className="text-slate-400">
                  {" "}
                  · {nameOf(parentOf(l.root_path))} · {l.files.toLocaleString()} files · {whyOf(c)[0] ?? ""}
                </span>
              </span>
              <span className="flex gap-2">
                <button type="button" className={primary} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "is_project", yes: true })}>
                  It's a project
                </button>
                <button type="button" className={btn} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "is_project", yes: false })}>
                  Not a project
                </button>
              </span>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

/** What a preview answer does to the sample data, so the screen can be tried in a browser. */
function previewAnswer(list: Collection[], c: Collection, a: Answer, locationId?: string): Collection[] {
  switch (a.kind) {
    case "is_project":
      return a.yes ? list.map((x) => (x.id === c.id ? { ...x, state: "confirmed" } : x)) : list.filter((x) => x.id !== c.id);
    case "leave_as_is":
      return list.map((x) => (x.id === c.id ? { ...x, state: "left_as_is" } : x));
    case "rename":
      return list.map((x) => (x.id === c.id ? { ...x, name: a.name } : x));
    case "not_same_as": {
      const l = c.locations.find((x) => x.id === locationId);
      if (!l) return list;
      const rest = list.map((x) => (x.id === c.id ? { ...x, locations: x.locations.filter((y) => y.id !== l.id) } : x));
      return [...rest, { ...c, id: `${c.id}-${l.id}`, name: nameOf(l.root_path), state: "candidate", locations: [{ ...l, relationship: "home" }] }];
    }
    default:
      return list;
  }
}

export function ProjectsView({ preview, onBack }: { preview: boolean; onBack: () => void }) {
  const [list, setList] = useState<Collection[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (preview) {
      setList((l) => l ?? fixtureProjects);
      return;
    }
    try {
      setList(await aegis.context.projects());
      setErr(null);
    } catch (e) {
      setErr(errorMessage(e));
    }
  }, [preview]);

  useEffect(() => {
    void load();
    if (!preview) void aegis.context.recognising().then(setBusy).catch(() => undefined);
  }, [load, preview]);

  // While a pass started elsewhere is running, check back until it is done.
  useEffect(() => {
    if (!busy || preview) return;
    const t = window.setInterval(() => {
      void aegis.context.recognising().then((r) => {
        if (!r) {
          setBusy(false);
          void load();
        }
      });
    }, 2000);
    return () => window.clearInterval(t);
  }, [busy, preview, load]);

  const run = async (work: () => Promise<void>) => {
    if (busy) return;
    setBusy(true);
    setNote(null);
    try {
      await work();
      await load();
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const ans: Answering = {
    busy,
    send: async (c, a, locationId) => {
      if (preview) {
        setList((l) => previewAnswer(l ?? [], c, a, locationId));
        return;
      }
      await run(() => aegis.context.answer(c.id, a, locationId));
    },
  };

  const lookAgain = () =>
    void run(async () => {
      if (preview) {
        await new Promise((r) => window.setTimeout(r, 800));
        return;
      }
      const rep = await aegis.context.recogniseNow();
      setNote(`Found ${rep.projects.toLocaleString()} projects in ${(rep.took_ms / 1000).toFixed(0)} s.`);
    });

  const groups = useMemo(() => {
    const all = list ?? [];
    const asks = (c: Collection) => c.state === "candidate" && c.confidence < SURE && c.locations.length === 1;
    return {
      multi: all.filter((c) => c.locations.length > 1),
      confirm: all.filter(asks),
      single: all.filter((c) => c.locations.length === 1 && !asks(c)),
    };
  }, [list]);

  return (
    <div className="space-y-4">
      <button type="button" onClick={onBack} className="flex items-center gap-1 text-xs text-emerald-300 hover:underline">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> Filing
      </button>
      <header className="flex flex-wrap items-end justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">Projects</h1>
          <p className="text-xs text-slate-400">
            {list
              ? `${list.length.toLocaleString()} projects · ${groups.multi.length} found in more than one place · filing keeps each one whole`
              : "Filing keeps each project whole."}
          </p>
        </div>
        <button type="button" className={btn} disabled={busy} onClick={lookAgain}>
          {busy ? (
            <>
              <Loader2 className="mr-1 inline h-3.5 w-3.5 animate-spin" aria-hidden="true" /> Looking…
            </>
          ) : (
            <>
              <RefreshCw className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Look again now
            </>
          )}
        </button>
      </header>
      {err && <p className="text-xs text-rose-300">{err}</p>}
      {note && (
        <p role="status" className="text-xs text-slate-300">
          {note}
        </p>
      )}
      {preview && <p className="text-xs text-slate-500">Preview with sample projects: answers here are not saved.</p>}
      {!list && !err && <p className="text-xs text-slate-400">Loading projects…</p>}
      {list && list.length === 0 && (
        <p className={`${card} p-4 text-xs text-slate-400`}>
          No projects found yet. Aegis looks for them by itself while your PC is quiet, or press Look again now.
        </p>
      )}
      <ConfirmList items={groups.confirm} ans={ans} />
      {groups.multi.map((c) => (
        <ProjectCard key={c.id} c={c} ans={ans} />
      ))}
      {groups.single.length > 0 && (
        <section aria-label="Projects in one place" className={`${card} p-4`}>
          <h2 className="text-xs font-semibold text-slate-300">In one place ({groups.single.length})</h2>
          <ul className="mt-2">
            {groups.single.map((c) => (
              <li key={c.id} className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 py-2 last:border-b-0">
                <span className="min-w-0 text-xs">
                  <span className="font-medium text-slate-100">{c.name}</span>{" "}
                  <span className="text-slate-400" title={c.locations[0].root_path}>
                    · {shortPath(c.locations[0].root_path, 3)} · {KIND_TEXT[c.project_kind ?? "other"]}
                  </span>{" "}
                  <SureChip c={c} />
                </span>
                <button type="button" className={btn} disabled={busy} onClick={() => void ans.send(c, { kind: "is_project", yes: false })}>
                  Not a project
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </div>
  );
}
