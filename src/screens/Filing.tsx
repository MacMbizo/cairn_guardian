import { useEffect, useMemo, useState } from "react";
import { ArrowLeft, ArrowRight, CheckCircle2, FolderGit2, FolderTree, HardDrive, History, Laptop, ListChecks, Sparkles, Undo2 } from "lucide-react";
import {
  aegis,
  errorMessage,
  type ActionItem,
  type BlueprintChoice,
  type BlueprintSummary,
  type FailedItem,
  type FilingRule,
  type Placement,
  type RecentBatch,
  type Scope,
  type ScopeStatus,
  type TreeNode,
} from "../bindings/client";
import { formatBytes } from "../format";
import { fixtureBatch, fixtureBlueprints, fixtureDestinations, fixtureRecent, fixtureRules, fixtureToSort } from "./fixtures/filing";
import type { FilingData } from "./useFiling";
import { ProjectsView } from "./Projects";

const card = "rounded-xl border border-slate-800 bg-slate-900/50 p-4";
const btn = "rounded-md border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40";
const primary = "rounded-md bg-emerald-600 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40";

type View =
  | { kind: "overview" }
  | { kind: "plan"; s: ScopeStatus }
  | { kind: "batch"; s: ScopeStatus; batch: string }
  | { kind: "sort"; s: ScopeStatus }
  | { kind: "rules" }
  | { kind: "recent" }
  | { kind: "projects" };

const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;
const folderOf = (p: string) => p.split(/[\\/]/).slice(0, -1).join("\\");

/** Shortens a path to its last few folders so rows stay readable. */
function shortFolder(p: string, keep = 4): string {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts.length <= keep ? parts.join("\\") : `…\\${parts.slice(-keep).join("\\")}`;
}

function Pill({ c }: { c: number }) {
  const sure = c >= 0.9;
  return (
    <span className={`shrink-0 rounded px-1.5 py-0.5 text-[11px] font-medium ${sure ? "bg-emerald-500/15 text-emerald-300" : "bg-amber-500/15 text-amber-300"}`}>
      {sure ? "Sure" : "Likely"} · {Math.round(c * 100)}%
    </span>
  );
}

function Tree({ nodes }: { nodes: TreeNode[] }) {
  return (
    <ul className="space-y-1 text-xs">
      {nodes.map((n) => (
        <li key={n.name}>
          <div className="flex items-center justify-between gap-2">
            <span className="flex items-center gap-1.5 font-medium text-slate-100">
              <FolderTree className="h-3.5 w-3.5 text-emerald-400" aria-hidden="true" />
              {n.name}
            </span>
            <span className="text-slate-400">
              {n.files.toLocaleString()} files · {formatBytes(n.bytes)}
            </span>
          </div>
          {n.children.length > 0 && (
            <ul className="ml-5 mt-0.5 space-y-0.5 border-l border-slate-800 pl-2">
              {n.children.slice(0, 6).map((c) => (
                <li key={c.name} className="flex items-center justify-between gap-2 text-slate-300">
                  <span>{c.name}</span>
                  <span className="text-slate-500">{c.files.toLocaleString()}</span>
                </li>
              ))}
              {n.children.length > 6 && <li className="text-slate-500">and {n.children.length - 6} more</li>}
            </ul>
          )}
        </li>
      ))}
    </ul>
  );
}

const CHOICE_TEXT: Record<BlueprintChoice, { title: string; blurb: string }> = {
  tidy: { title: "Tidy", blurb: "Your own folders, cleaned up: look-alike folders merged, loose files put away, clear names." },
  fresh: { title: "Fresh", blurb: "The standard structure on every drive: Documents, Photos, Videos, Music, Software — by subject and year." },
};

function PlanView({ s, preview, onChosen, onBack }: { s: ScopeStatus; preview: boolean; onChosen: (msg: string) => void; onBack: () => void }) {
  const [pair, setPair] = useState<[BlueprintSummary, BlueprintSummary] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    if (preview) {
      setPair(fixtureBlueprints);
      return;
    }
    aegis.filing.blueprints(s.scope).then(setPair, (e) => setErr(errorMessage(e)));
  }, [s, preview]);

  const [err2, setErr2] = useState<string | null>(null);
  const choose = async (c: BlueprintChoice) => {
    try {
      if (!preview) await aegis.filing.choose(s.scope, c);
    } catch (e) {
      setErr2(errorMessage(e));
      return;
    }
    const switched = s.blueprint !== null && s.blueprint !== c;
    onChosen(
      switched
        ? `${s.label} now uses the ${CHOICE_TEXT[c].title.toLowerCase()} structure. Files already filed stayed where they are; the next batch follows the new structure.`
        : `${s.label} will use the ${CHOICE_TEXT[c].title.toLowerCase()} structure. Review the first batch when you're ready.`,
    );
  };
  // Switching after batches have run: say what happens to the work already done.
  const midway = s.blueprint !== null && s.filed > 0;

  return (
    <div className="space-y-4">
      <button type="button" onClick={onBack} className="flex items-center gap-1 text-xs text-emerald-300 hover:underline">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> All drives
      </button>
      <header>
        <h1 className="text-lg font-semibold">Choose a structure for {s.label}</h1>
        <p className="text-xs text-slate-400">Nothing moves yet. After you choose, Aegis proposes batches you approve one at a time, and every batch can be undone.</p>
      </header>
      {midway && s.blueprint && (
        <p role="note" className="rounded-lg border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-xs text-amber-200">
          {s.filed.toLocaleString()} file{s.filed === 1 ? " has" : "s have"} already been filed under the {CHOICE_TEXT[s.blueprint].title.toLowerCase()} structure.
          Switching won't move them back or again: they stay where they are, and only files not yet filed follow the new choice
          {s.open_batch ? ". The batch waiting for you now is dropped" : ""}. To start over, undo batches from Recent batches first.
        </p>
      )}
      {(err ?? err2) && <p className="text-xs text-rose-300">{err ?? err2}</p>}
      {!pair && !err && <p className="text-xs text-slate-400">Working out both structures… this can take a minute on a big drive.</p>}
      {pair && (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
          {pair.map((b) => (
            <section key={b.choice} aria-label={CHOICE_TEXT[b.choice].title} className={`${card} flex flex-col`}>
              <h2 className="text-sm font-semibold">{CHOICE_TEXT[b.choice].title}</h2>
              <p className="mb-3 text-xs text-slate-400">{CHOICE_TEXT[b.choice].blurb}</p>
              <div className="mb-3 flex flex-wrap gap-2 text-xs">
                <span className="rounded bg-slate-800 px-2 py-0.5">Moves {b.moves.toLocaleString()}</span>
                <span className="rounded bg-slate-800 px-2 py-0.5">Renames {b.renames.toLocaleString()}</span>
                <span className="rounded bg-slate-800 px-2 py-0.5">To sort {b.to_sort.toLocaleString()}</span>
                <span className="rounded bg-slate-800 px-2 py-0.5 text-slate-400">Stay put {b.stays.toLocaleString()}</span>
              </div>
              <div className="mb-3 flex-1">
                <Tree nodes={b.tree} />
              </div>
              {b.examples.length > 0 && (
                <div className="mb-3 rounded-lg bg-slate-950/60 p-2 text-xs">
                  <p className="mb-1 text-slate-400">For example</p>
                  <ul className="space-y-0.5">
                    {b.examples.map(([from, to]) => (
                      <li key={`${from}-${to}`} className="flex flex-wrap items-center gap-1.5">
                        <span className="text-slate-400">{from}</span>
                        <ArrowRight className="h-3 w-3 text-slate-500" aria-hidden="true" />
                        <span className="text-slate-100">{to}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
              <button type="button" className={primary} onClick={() => void choose(b.choice)}>
                {s.blueprint === b.choice ? "Keep this structure" : midway ? "Switch to this structure" : "Use this structure"}
              </button>
            </section>
          ))}
        </div>
      )}
    </div>
  );
}

function BatchView({
  s,
  batch,
  preview,
  onDone,
  onBack,
}: {
  s: ScopeStatus;
  batch: string;
  preview: boolean;
  onDone: (msg: string, undo?: string, failed?: FailedItem[]) => void;
  onBack: () => void;
}) {
  const [items, setItems] = useState<ActionItem[] | null>(null);
  const [ticked, setTicked] = useState<Set<string>>(new Set());
  const [why, setWhy] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    const load = preview ? Promise.resolve(fixtureBatch) : aegis.actions.list().then((all) => all.filter((a) => a.batch_id === batch));
    load.then(
      (list) => {
        setItems(list);
        setTicked(new Set(list.map((a) => a.id)));
      },
      (e) => setErr(errorMessage(e))
    );
  }, [batch, preview]);

  const approve = async (all: boolean) => {
    if (!items) return;
    if (preview) {
      onDone(`Filed ${all ? items.length : ticked.size} files.`, batch);
      return;
    }
    setBusy(true);
    try {
      const sel = all ? undefined : [...ticked];
      const res = await aegis.filing.approve(batch, sel);
      const parts = [`Filed ${res.filed.toLocaleString()} file${res.filed === 1 ? "" : "s"}`];
      if (res.skipped > 0) parts.push(`${res.skipped.toLocaleString()} left where ${res.skipped === 1 ? "it is" : "they are"}`);
      if (res.failed.length > 0) parts.push(`${res.failed.length.toLocaleString()} couldn't move`);
      onDone(`${parts.join(" · ")}.`, batch, res.failed);
    } catch (e) {
      setErr(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const skip = async () => {
    try {
      if (!preview) await aegis.filing.skip(batch);
      onDone("Skipped. Those files stay where they are and won't be suggested again.");
    } catch (e) {
      setErr(errorMessage(e));
    }
  };

  return (
    <div className="space-y-4">
      <button type="button" onClick={onBack} className="flex items-center gap-1 text-xs text-emerald-300 hover:underline">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> All drives
      </button>
      <header className="flex flex-wrap items-end justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">Next batch for {s.label}</h1>
          <p className="text-xs text-slate-400">Untick anything you'd rather leave where it is. Aegis remembers, and learns from what you approve.</p>
        </div>
        <div className="flex gap-2">
          <button type="button" className={btn} disabled={busy} onClick={() => void skip()}>
            Skip this folder
          </button>
          <button type="button" className={btn} disabled={busy || ticked.size === 0} onClick={() => void approve(false)}>
            Approve ticked ({ticked.size})
          </button>
          <button type="button" className={primary} disabled={busy || !items?.length} onClick={() => void approve(true)}>
            Approve all
          </button>
        </div>
      </header>
      {err && <p className="text-xs text-rose-300">{err}</p>}
      {!items && !err && <p className="text-xs text-slate-400">Loading…</p>}
      {items && (
        <ul className={`${card} divide-y divide-slate-800 p-0`}>
          {items.map((a) => {
            const renamed = a.to_path && fileName(a.to_path) !== fileName(a.from_path);
            return (
              <li key={a.id} className="px-4 py-2.5">
                <div className="flex items-start gap-3">
                  <input
                    type="checkbox"
                    aria-label={`File ${fileName(a.from_path)}`}
                    checked={ticked.has(a.id)}
                    onChange={(e) => {
                      const next = new Set(ticked);
                      if (e.target.checked) next.add(a.id);
                      else next.delete(a.id);
                      setTicked(next);
                    }}
                    className="mt-1 h-4 w-4 accent-emerald-500"
                  />
                  <div className="min-w-0 flex-1 text-xs">
                    <div className="flex flex-wrap items-center gap-1.5">
                      <span className={renamed ? "text-slate-400 line-through decoration-slate-600" : "text-slate-100"}>{fileName(a.from_path)}</span>
                      {renamed && (
                        <>
                          <ArrowRight className="h-3 w-3 text-slate-500" aria-hidden="true" />
                          <span className="font-medium text-emerald-200">{fileName(a.to_path ?? "")}</span>
                        </>
                      )}
                    </div>
                    <div className="mt-0.5 flex flex-wrap items-center gap-1.5 text-slate-500">
                      <span>{shortFolder(folderOf(a.from_path))}</span>
                      <ArrowRight className="h-3 w-3" aria-hidden="true" />
                      <span className="text-slate-300">{shortFolder(folderOf(a.to_path ?? ""))}</span>
                    </div>
                    {why === a.id && <p className="mt-1 rounded bg-slate-950/60 px-2 py-1 text-slate-300">{a.reason}</p>}
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    <Pill c={a.confidence} />
                    <button type="button" className="text-xs text-slate-400 hover:text-slate-200" onClick={() => setWhy(why === a.id ? null : a.id)}>
                      Why?
                    </button>
                  </div>
                </div>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

function SortView({ s, preview, onBack }: { s: ScopeStatus; preview: boolean; onBack: () => void }) {
  const [items, setItems] = useState<Placement[] | null>(null);
  const [dests, setDests] = useState<Record<string, string[]>>({});
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    if (preview) {
      setItems(fixtureToSort);
      setDests(Object.fromEntries(fixtureToSort.flatMap((p) => (p.target.type === "file" ? [[p.target.file_id, fixtureDestinations]] : []))));
      return;
    }
    void aegis.filing.toSort(s.scope).then((list) => {
      setItems(list);
      const ids = list.flatMap((p) => (p.target.type === "file" ? [p.target.file_id] : []));
      if (ids.length > 0) void aegis.filing.destinations(s.scope, ids).then(setDests);
    });
  }, [s, preview]);

  const place = async (p: Placement, dest: string) => {
    if (p.target.type !== "file") return;
    try {
      if (!preview) await aegis.filing.place(s.scope, p.target.file_id, dest);
      setItems((list) => list?.filter((x) => x !== p) ?? null);
      setMsg(`Moved ${fileName(p.from)} to ${shortFolder(dest, 3)}.`);
    } catch (e) {
      setMsg(errorMessage(e));
    }
  };

  return (
    <div className="space-y-4">
      <button type="button" onClick={onBack} className="flex items-center gap-1 text-xs text-emerald-300 hover:underline">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> All drives
      </button>
      <header>
        <h1 className="text-lg font-semibold">To sort on {s.label}</h1>
        <p className="text-xs text-slate-400">Aegis wasn't sure about these. Pick a place, or leave them where they are.</p>
      </header>
      {msg && <p role="status" className="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-200">{msg}</p>}
      {items?.length === 0 && <p className="text-xs text-slate-400">Nothing left to sort.</p>}
      <ul className={`${card} divide-y divide-slate-800 p-0`}>
        {items?.map((p) => (
          <li key={p.from} className="flex flex-wrap items-center justify-between gap-2 px-4 py-2.5 text-xs">
            <span className="min-w-0">
              <span className="block text-slate-100">{fileName(p.from)}</span>
              <span className="block text-slate-500">{shortFolder(folderOf(p.from))} · {p.reason}</span>
            </span>
            <span className="flex flex-wrap gap-1.5">
              {(p.target.type === "file" ? (dests[p.target.file_id] ?? []) : []).map((d) => (
                <button key={d} type="button" className={btn} onClick={() => void place(p, d)}>
                  {shortFolder(d, 2)}
                </button>
              ))}
              <button type="button" className="rounded-md px-2 py-1 text-xs text-slate-400 hover:bg-slate-800" onClick={() => setItems((l) => l?.filter((x) => x !== p) ?? null)}>
                Leave here
              </button>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function ruleLabel(r: FilingRule): string {
  const what = r.category ? r.category.split("/").pop() : r.kind === "installer" ? "Installers" : r.kind[0].toUpperCase() + r.kind.slice(1);
  return r.who ? `${what} from ${r.who}` : (what ?? r.kind);
}

const whenText = (ms: number) =>
  new Date(ms).toLocaleString(undefined, { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });

function RecentView({
  scopes,
  preview,
  onUndo,
  onBack,
}: {
  scopes: ScopeStatus[];
  preview: boolean;
  onUndo: (batch: string) => Promise<void>;
  onBack: () => void;
}) {
  const [list, setList] = useState<RecentBatch[] | null>(null);
  const [undoing, setUndoing] = useState<string | null>(null);
  const load = () => (preview ? Promise.resolve(fixtureRecent) : aegis.filing.recent()).then(setList);
  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preview]);
  const label = (sc: Scope) => scopes.find((x) => scopeKey(x.scope) === scopeKey(sc))?.label ?? (sc.type === "pc" ? "This PC" : "a drive");
  const undo = async (id: string) => {
    if (undoing) return;
    setUndoing(id);
    try {
      await onUndo(id);
      await load();
    } finally {
      setUndoing(null);
    }
  };
  return (
    <div className="space-y-4">
      <button type="button" onClick={onBack} className="flex items-center gap-1 text-xs text-emerald-300 hover:underline">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> All drives
      </button>
      <header>
        <h1 className="text-lg font-semibold">Recent batches</h1>
        <p className="text-xs text-slate-400">Everything filing has moved, including what Aegis filed by itself. Any batch can be put back.</p>
      </header>
      {list?.length === 0 && <p className="text-xs text-slate-400">Nothing has been filed yet.</p>}
      <ul className={`${card} divide-y divide-slate-800 p-0`}>
        {list?.map((b) => (
          <li key={b.id} className="flex flex-wrap items-center justify-between gap-2 px-4 py-2.5 text-xs">
            <span className="min-w-0">
              <span className="block font-medium text-slate-100">
                {b.project
                  ? `Project plan: ${b.project} (${b.files.toLocaleString()} folder${b.files === 1 ? "" : "s"})`
                  : `${b.files.toLocaleString()} item${b.files === 1 ? "" : "s"} on ${label(b.scope)}`}
                {b.automatic && (
                  <span className="ml-2 inline-flex items-center gap-1 rounded bg-emerald-500/15 px-1.5 py-0.5 font-normal text-emerald-300">
                    <Sparkles className="h-3 w-3" aria-hidden="true" /> Filed by itself
                  </span>
                )}
              </span>
              <span className="block text-slate-500">{whenText(b.at)}</span>
            </span>
            {b.can_undo ? (
              <button type="button" className={btn} disabled={undoing !== null} onClick={() => void undo(b.id)}>
                <Undo2 className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />
                {undoing === b.id ? "Undoing…" : "Undo"}
              </button>
            ) : (
              <span className="text-slate-500">Undone</span>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

function RulesView({ preview, onBack }: { preview: boolean; onBack: () => void }) {
  const [rules, setRules] = useState<FilingRule[] | null>(null);
  const load = () => (preview ? Promise.resolve(fixtureRules) : aegis.filing.rules()).then(setRules);
  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preview]);
  const act = async (fn: () => Promise<void>) => {
    if (!preview) await fn();
    await load();
  };
  return (
    <div className="space-y-4">
      <button type="button" onClick={onBack} className="flex items-center gap-1 text-xs text-emerald-300 hover:underline">
        <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" /> All drives
      </button>
      <header>
        <h1 className="text-lg font-semibold">What Aegis has learned</h1>
        <p className="text-xs text-slate-400">
          After you approve the same kind of move three times, Aegis files new ones by itself. One "no" and it asks again.
        </p>
      </header>
      {rules?.length === 0 && <p className="text-xs text-slate-400">Nothing yet. Rules appear as you approve batches.</p>}
      <ul className={`${card} divide-y divide-slate-800 p-0`}>
        {rules?.map((r) => (
          <li key={r.id} className="flex flex-wrap items-center justify-between gap-2 px-4 py-2.5 text-xs">
            <span className="min-w-0">
              <span className="block font-medium text-slate-100">{ruleLabel(r)}</span>
              <span className="block text-slate-500">
                Goes to {r.dest_template || "its folder"}
                {r.rename ? " · renamed" : ""}
              </span>
            </span>
            <span className="flex items-center gap-2">
              {r.state === "automatic" ? (
                <span className="flex items-center gap-1 rounded bg-emerald-500/15 px-1.5 py-0.5 text-emerald-300">
                  <Sparkles className="h-3 w-3" aria-hidden="true" /> Automatic
                </span>
              ) : (
                <span className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300">Asking · {Math.min(r.approvals, 3)} of 3 approvals</span>
              )}
              {r.state === "automatic" && (
                <button type="button" className={btn} onClick={() => void act(() => aegis.filing.stopRule(r.id))}>
                  Stop automatic
                </button>
              )}
              <button type="button" className="rounded-md px-2 py-1 text-xs text-rose-300 hover:bg-rose-950/40" onClick={() => void act(() => aegis.filing.deleteRule(r.id))}>
                Delete
              </button>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

const scopeKey = (s: Scope) => (s.type === "pc" ? "pc" : s.volume_id);

/** Filing (spec §14): structure per drive, batch review, to sort, learned rules. */
export function FilingScreen({ data, startRecent = false }: { data: FilingData; startRecent?: boolean }) {
  const { scopes, loading, error, preview, reload } = data;
  const [view, setView] = useState<View>(startRecent ? { kind: "recent" } : { kind: "overview" });
  const [msg, setMsg] = useState<{ text: string; undo?: string; failed?: FailedItem[] } | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const back = () => {
    setView({ kind: "overview" });
    void reload();
  };
  const openBatch = async (s: ScopeStatus) => {
    if (preview) {
      setView({ kind: "batch", s, batch: s.open_batch ?? "fb1" });
      return;
    }
    setBusy(scopeKey(s.scope));
    try {
      const b = s.open_batch ?? (await aegis.filing.nextBatch(s.scope));
      if (b) setView({ kind: "batch", s, batch: b });
      else setMsg({ text: `Nothing left to file on ${s.label}.` });
    } catch (e) {
      setMsg({ text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };
  const undo = async (batch: string) => {
    try {
      const res = preview ? null : await aegis.filing.undo(batch);
      const stuck = res?.conflicts.length ?? 0;
      setMsg({
        text:
          stuck === 0
            ? "Undone. Everything is back where it was."
            : `Undone except ${stuck} file${stuck === 1 ? "" : "s"} that changed since (something new is in the old place). Open Simulator & Actions to choose what happens to ${stuck === 1 ? "it" : "them"}.`,
      });
      void reload();
    } catch (e) {
      setMsg({ text: errorMessage(e) });
    }
  };

  const totals = useMemo(() => scopes.reduce((t, s) => t + s.filed, 0), [scopes]);

  if (view.kind === "plan")
    return <PlanView s={view.s} preview={preview} onBack={back} onChosen={(text) => { setMsg({ text }); back(); }} />;
  if (view.kind === "batch")
    return <BatchView s={view.s} batch={view.batch} preview={preview} onBack={back} onDone={(text, b, failed) => { setMsg({ text, undo: b, failed }); back(); }} />;
  if (view.kind === "sort") return <SortView s={view.s} preview={preview} onBack={back} />;
  if (view.kind === "rules") return <RulesView preview={preview} onBack={back} />;
  if (view.kind === "recent") return <RecentView scopes={scopes} preview={preview} onUndo={undo} onBack={back} />;
  if (view.kind === "projects") return <ProjectsView preview={preview} onBack={back} />;

  return (
    <div className="space-y-4">
      <header className="flex flex-wrap items-end justify-between gap-2">
        <div>
          <h1 className="text-lg font-semibold">Filing</h1>
          <p className="text-xs text-slate-400">
            One clear structure per drive, good names for documents, and nothing moves without you.
            {totals > 0 && ` ${totals.toLocaleString()} files filed so far.`}
          </p>
        </div>
        <span className="flex flex-wrap gap-2">
          <button type="button" className={btn} onClick={() => setView({ kind: "projects" })}>
            <FolderGit2 className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Projects
          </button>
          <button type="button" className={btn} onClick={() => setView({ kind: "recent" })}>
            <History className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Recent batches
          </button>
          <button type="button" className={btn} onClick={() => setView({ kind: "rules" })}>
            <ListChecks className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> What Aegis has learned
          </button>
        </span>
      </header>
      {msg && (
        <p role="status" className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-200">
          <span className="flex items-center gap-1.5">
            <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" aria-hidden="true" />
            {msg.text}
          </span>
          {msg.undo && (
            <button type="button" className={btn} onClick={() => void undo(msg.undo ?? "")}>
              <Undo2 className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Undo
            </button>
          )}
        </p>
      )}
      {msg?.failed && msg.failed.length > 0 && (
        <details className="rounded-lg border border-amber-500/40 bg-amber-500/5 px-3 py-2 text-xs text-slate-200" open={msg.failed.length <= 5}>
          <summary className="cursor-pointer text-amber-200">
            {msg.failed.length.toLocaleString()} couldn't move. They're still where they were.
          </summary>
          <ul className="mt-1.5 space-y-0.5">
            {msg.failed.slice(0, 50).map((f) => (
              <li key={f.name}>
                <span className="text-slate-100">{fileName(f.name)}</span>
                <span className="text-slate-400"> · {f.reason}</span>
              </li>
            ))}
            {msg.failed.length > 50 && <li className="text-slate-500">…and {(msg.failed.length - 50).toLocaleString()} more</li>}
          </ul>
        </details>
      )}
      {error && <p className="text-xs text-rose-300">{error}</p>}
      {loading && <p className="text-xs text-slate-400">Loading…</p>}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-2">
        {scopes.map((s) => {
          const Icon = s.scope.type === "pc" ? Laptop : HardDrive;
          const reading = s.read_pct < 0.999;
          return (
            <section key={scopeKey(s.scope)} aria-label={s.label} className={card}>
              <div className="mb-2 flex items-center justify-between gap-2">
                <h2 className="flex items-center gap-2 text-sm font-semibold">
                  <Icon className="h-4 w-4 text-emerald-400" aria-hidden="true" /> {s.label}
                </h2>
                <span className="text-xs text-slate-400">
                  {s.blueprint ? `${CHOICE_TEXT[s.blueprint].title} structure` : "Not planned yet"}
                  {!s.online && " · unplugged"}
                </span>
              </div>
              {reading && (
                <div className="mb-2">
                  <div className="mb-1 flex justify-between text-xs text-slate-400">
                    <span>Reading files so Aegis knows what they are</span>
                    <span>{Math.round(s.read_pct * 100)}%</span>
                  </div>
                  <div className="h-1.5 overflow-hidden rounded bg-slate-800">
                    <div className="h-full bg-emerald-500" style={{ width: `${Math.round(s.read_pct * 100)}%` }} />
                  </div>
                </div>
              )}
              {s.blueprint && (
                <p className="mb-3 text-xs text-slate-300">
                  Filed {s.filed.toLocaleString()}
                  {s.counts_known ? ` · ${s.remaining.toLocaleString()} still to move` : " · plug the drive in to see what's left"}
                  {s.to_sort > 0 && (
                    <>
                      {" · "}
                      <button type="button" className="text-amber-300 hover:underline" onClick={() => setView({ kind: "sort", s })}>
                        {s.to_sort.toLocaleString()} to sort
                      </button>
                    </>
                  )}
                </p>
              )}
              <div className="flex flex-wrap gap-2">
                {!s.blueprint ? (
                  <button type="button" className={primary} disabled={!s.online} onClick={() => setView({ kind: "plan", s })}>
                    Plan this {s.scope.type === "pc" ? "PC" : "drive"}
                  </button>
                ) : !s.counts_known ? (
                  <span className="text-xs text-slate-400">Plug in to see</span>
                ) : s.open_batch || s.remaining > 0 ? (
                  <button type="button" className={primary} disabled={!s.online || busy !== null} onClick={() => void openBatch(s)}>
                    {busy === scopeKey(s.scope) ? "Preparing…" : "Review next batch"}
                  </button>
                ) : (
                  <span className="flex items-center gap-1 text-xs text-emerald-300">
                    <CheckCircle2 className="h-3.5 w-3.5" aria-hidden="true" /> All filed
                  </span>
                )}
                {s.blueprint && (
                  <button type="button" className={btn} disabled={!s.online} onClick={() => setView({ kind: "plan", s })}>
                    Change structure
                  </button>
                )}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}
