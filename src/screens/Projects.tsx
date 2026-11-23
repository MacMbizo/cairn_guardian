import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { ArrowLeft, ArrowRight, BookOpen, CircleCheck, Code2, Copy, FileText, Folder, GitBranch, HardDrive, History, House, Layers, Loader2, Palette, RefreshCw, SearchCheck } from "lucide-react";
import {
  aegis,
  errorMessage,
  type Answer,
  type Collection,
  type CollectionKind,
  type Location,
  type PlanOutcome,
  type PlanStep,
  type ProjectKind,
  type ProjectPlan,
  type Relationship,
  type ReunitePlan,
} from "../bindings/client";
import { fixturePlans, fixtureProjects, fixtureReunitePlans } from "./fixtures/projects";

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

const KIND_ORDER: ProjectKind[] = ["software", "design", "writing", "research", "other"];

const KIND_META: Record<ProjectKind, { label: string; tone: string; pill: string; icon: ReactNode }> = {
  software: {
    label: "Software",
    tone: "bg-sky-500/15 text-sky-300",
    pill: "border-sky-500/50 bg-sky-500/15 text-sky-200",
    icon: <Code2 className="h-3 w-3 shrink-0" aria-hidden="true" />,
  },
  design: {
    label: "Design",
    tone: "bg-fuchsia-500/15 text-fuchsia-300",
    pill: "border-fuchsia-500/50 bg-fuchsia-500/15 text-fuchsia-200",
    icon: <Palette className="h-3 w-3 shrink-0" aria-hidden="true" />,
  },
  writing: {
    label: "Writing",
    tone: "bg-amber-500/15 text-amber-300",
    pill: "border-amber-500/50 bg-amber-500/15 text-amber-200",
    icon: <FileText className="h-3 w-3 shrink-0" aria-hidden="true" />,
  },
  research: {
    label: "Research",
    tone: "bg-violet-500/15 text-violet-300",
    pill: "border-violet-500/50 bg-violet-500/15 text-violet-200",
    icon: <BookOpen className="h-3 w-3 shrink-0" aria-hidden="true" />,
  },
  other: {
    label: "Other",
    tone: "bg-slate-800 text-slate-300",
    pill: "border-slate-600 bg-slate-800 text-slate-100",
    icon: <Folder className="h-3 w-3 shrink-0" aria-hidden="true" />,
  },
};

function KindBadge({ kind }: { kind: ProjectKind | null | undefined }) {
  const m = KIND_META[kind ?? "other"];
  return (
    <Chip tone={`${m.tone} inline-flex items-center gap-1`}>
      {m.icon}
      {m.label}
    </Chip>
  );
}

function KindSelect({ c, ans }: { c: Collection; ans: Answering }) {
  const val = c.project_kind ?? "other";
  return (
    <select
      aria-label={`Project type for ${c.name}`}
      value={val}
      disabled={ans.busy}
      onChange={(e) => void ans.send(c, { kind: "set_kind", project_kind: e.target.value as ProjectKind })}
      className="rounded border border-slate-700 bg-slate-900 px-1.5 py-0.5 text-[11px] font-medium text-slate-200 hover:border-slate-500 disabled:opacity-40"
    >
      {KIND_ORDER.map((k) => (
        <option key={k} value={k}>
          {KIND_META[k].label}
        </option>
      ))}
    </select>
  );
}

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
    set_files: {
      label: "Loose set files",
      detail: `${l.files.toLocaleString()} matching files in this folder`,
      icon: <Layers className="h-4 w-4" aria-hidden="true" />,
      tone: "text-sky-300",
    },
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

function formatBytes(b: number): string {
  if (b >= 1e9) return `${(b / 1e9).toFixed(1)} GB`;
  if (b >= 1e6) return `${(b / 1e6).toFixed(0)} MB`;
  return `${Math.max(1, Math.round(b / 1e3))} KB`;
}

/** A project's plan on one drive. */
const planOn = (plans: ProjectPlan[], c: Collection, volumeId: string) => plans.find((p) => p.collection_id === c.id && p.volume_id === volumeId);

/** The plan on the main copy's drive: the one the card approves. */
function planOf(plans: ProjectPlan[], c: Collection): ProjectPlan | undefined {
  const home = c.locations.find((l) => l.relationship === "home") ?? c.locations[0];
  return home ? planOn(plans, c, home.volume_id) : undefined;
}

/** The step that decides where a place ends up (a copy moved aside first has two). */
const stepFor = (p: ProjectPlan | undefined, l: Location) => p?.steps.filter((s) => s.location_id === l.id).pop();

function lineOf(s: PlanStep): string {
  switch (s.kind) {
    case "move_home": {
      const inDesign = /[\\/]design[\\/][^\\/]+$/i.test(s.to);
      return `moves whole to ${s.to} (${inDesign ? "your design projects folder" : "your projects home"})`;
    }
    case "quarantine":
      return "to quarantine for 30 days: all of it is in the main copy";
    case "to_versions":
      return `kept whole as a version in ${shortPath(s.to, 3)}`;
    case "to_material":
      return `brought into ${shortPath(s.to, 3)}`;
    case "gather_files":
      return `moves matching files into ${shortPath(s.to, 3)}`;
    default:
      return s.reason;
  }
}

/** What the card says about a place the plan leaves alone, if anything. */
function keptNote(plan: ProjectPlan | undefined, l: Location): string | null {
  if (l.keep_here && l.relationship !== "home") return "stays here, as you asked";
  const note = plan?.notes.find((n) => n.toLowerCase().startsWith(l.root_path.toLowerCase()));
  if (note) return note.slice(l.root_path.length).replace(/^\s*/, "");
  if (plan && plan.steps.length === 0 && plan.summary.includes("later")) return "stays where it is for now: plans for other drives come later";
  return null;
}

/** A plan run that finished on this screen, or was undone from it. */
type Outcome = PlanOutcome & { undone?: boolean };

interface Planning {
  plans: ProjectPlan[];
  /** The project whose plan is running now. */
  running: string | null;
  outcomes: Record<string, Outcome>;
  approve: (c: Collection, p: ProjectPlan) => void;
  undo: (c: Collection, batch: string) => void;
}

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

function PlaceRow({ c, l, ans, plan, choosing, onPicked }: { c: Collection; l: Location; ans: Answering; plan?: ProjectPlan; choosing: boolean; onPicked: () => void }) {
  const r = relation(l);
  const home = l.relationship === "home";
  const step = stepFor(plan, l);
  const kept = step ? null : keptNote(plan, l);
  const settled = c.state === "left_as_is";
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
          {step && !settled && (
            <span
              className="mt-0.5 flex items-start gap-1 text-[11px] text-slate-200"
              title={step.fallback ? `${step.reason} If anything in it isn't in the main copy, it is kept as a version instead.` : step.reason}
            >
              <ArrowRight className="mt-0.5 h-3 w-3 shrink-0 text-emerald-300" aria-hidden="true" />
              <span className="min-w-0 break-words">{lineOf(step)}</span>
            </span>
          )}
          {step && !settled && step.warnings.map((w) => (
            <span key={w} className="mt-0.5 block text-[11px] text-amber-300">
              {w}
            </span>
          ))}
          {kept && !settled && <span className="mt-0.5 block text-[11px] text-slate-400">→ {kept}</span>}
        </span>
      </span>
      {!home && !settled && (
        <span className="flex shrink-0 flex-wrap gap-1.5">
          {choosing && l.online ? (
            <button type="button" className={btn} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "change_home" }, l.id).then(onPicked)}>
              Make main copy
            </button>
          ) : (
            <>
              {step && !l.keep_here && (
                <button type="button" className={btn} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "keep_here", keep: true }, l.id)}>
                  Keep here
                </button>
              )}
              {l.keep_here && (
                <button type="button" className={btn} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "keep_here", keep: false }, l.id)}>
                  Include in plan
                </button>
              )}
              <button type="button" className={btn} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "not_same_as" }, l.id)}>
                Not this project
              </button>
            </>
          )}
        </span>
      )}
    </li>
  );
}

/** Under the places: what the plan does in one line, and Approve; or how it ended. */
function PlanFooter({ c, plan, pl, ans, choosing, setChoosing }: {
  c: Collection;
  plan?: ProjectPlan;
  pl: Planning;
  ans: Answering;
  choosing: boolean;
  setChoosing: (v: boolean) => void;
}) {
  const out = pl.outcomes[c.id];
  if (out?.undone) return <p className="mt-auto pt-2 text-xs text-slate-300">Undone: everything is back where it was.</p>;
  if (out?.done) {
    return (
      <div role="status" className="mt-auto flex flex-wrap items-center justify-between gap-2 rounded-lg border border-emerald-800 bg-emerald-950/40 px-3 py-2">
        <span className="flex items-center gap-1.5 text-xs text-emerald-200">
          <CircleCheck className="h-4 w-4" aria-hidden="true" />
          Done: {out.moved_files.toLocaleString()} files moved
          {out.freed_bytes > 0 && ` · ${formatBytes(out.freed_bytes)} to free in 30 days`}
        </span>
        <button type="button" className={btn} disabled={pl.running !== null} onClick={() => pl.undo(c, out.batch_id)}>
          Undo
        </button>
      </div>
    );
  }
  if (c.state === "left_as_is" || !plan) return null;
  const failed = out && !out.done ? <p className="text-xs text-rose-300">{out.reason}</p> : null;
  if (plan.waits_for) return <p className="mt-auto pt-2 text-xs text-slate-400">The plan waits: {plan.waits_for}</p>;
  if (plan.steps.length === 0) {
    return c.locations.length > 1 ? <p className="mt-auto pt-2 text-xs text-slate-400">{plan.summary}</p> : null;
  }
  const running = pl.running === c.id;
  return (
    <div className="mt-auto space-y-2 border-t border-slate-800 pt-3">
      {failed}
      <p className="text-xs text-slate-300">{plan.summary} One undo puts all of it back.</p>
      <div className="flex flex-wrap gap-2">
        <button type="button" className={primary} disabled={pl.running !== null || ans.busy || choosing} onClick={() => pl.approve(c, plan)}>
          {running ? (
            <>
              <Loader2 className="mr-1 inline h-3.5 w-3.5 animate-spin" aria-hidden="true" /> Moving…
            </>
          ) : (
            "Approve plan"
          )}
        </button>
        {c.locations.length > 1 && c.kind !== "design_set" && (
          <button type="button" className={btn} disabled={pl.running !== null || ans.busy} onClick={() => setChoosing(!choosing)}>
            {choosing ? "Cancel" : "Change main copy"}
          </button>
        )}
      </div>
      {choosing && <p className="text-[11px] text-slate-400">Pick the folder that should be the main copy.</p>}
    </div>
  );
}

function ProjectCard({
  c,
  otherProjects,
  ans,
  pl,
}: {
  c: Collection;
  otherProjects: Collection[];
  ans: Answering;
  pl: Planning;
}) {
  const [renaming, setRenaming] = useState(false);
  const [choosing, setChoosing] = useState(false);
  const [pickingSame, setPickingSame] = useState(false);
  const [sameTarget, setSameTarget] = useState("");
  const [name, setName] = useState(c.name);
  const home = c.kind === "design_set" ? undefined : (c.locations.find((l) => l.relationship === "home") ?? c.locations[0]);
  const others = home ? c.locations.filter((l) => l !== home) : c.locations;
  const isDesign = c.project_kind === "design";
  return (
    <section aria-label={c.name} className={`${card}${isDesign ? " border-l-2 border-l-fuchsia-500/70" : ""} grid grid-cols-1 md:grid-cols-[240px_minmax(0,1fr)]`}>
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
          <div className="flex flex-wrap items-center gap-1.5">
            <KindBadge kind={c.project_kind} />
            <KindSelect c={c} ans={ans} />
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
            <button type="button" className={planOf(pl.plans, c)?.steps.length ? btn : primary} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "leave_as_is" })}>
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
          {c.kind === "project" && otherProjects.length > 0 && (
            <div className="space-y-1.5">
              <button
                type="button"
                className={`${btn} w-full`}
                disabled={ans.busy}
                onClick={() => {
                  setPickingSame((v) => !v);
                  setSameTarget((prev) => prev || otherProjects[0].id);
                }}
              >
                {pickingSame ? "Cancel" : "Same project…"}
              </button>
              {pickingSame && (
                <form
                  className="flex gap-1.5"
                  onSubmit={(e) => {
                    e.preventDefault();
                    if (!sameTarget) return;
                    void ans.send(c, { kind: "same_as", other_collection_id: sameTarget }).then(() => setPickingSame(false));
                  }}
                >
                  <select
                    aria-label={`Merge ${c.name} with another project`}
                    value={sameTarget}
                    disabled={ans.busy}
                    onChange={(e) => setSameTarget(e.target.value)}
                    className="min-w-0 flex-1 rounded-md border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-200"
                  >
                    {otherProjects.map((o) => (
                      <option key={o.id} value={o.id}>
                        {o.name}
                      </option>
                    ))}
                  </select>
                  <button type="submit" className={primary} disabled={ans.busy || !sameTarget}>
                    Merge
                  </button>
                </form>
              )}
            </div>
          )}
        </div>
      </div>
      <div className="flex flex-col gap-2 p-4">
        <p className="text-[11px] font-semibold uppercase tracking-wide text-slate-400">Where it lives</p>
        <ul className="flex flex-col gap-1">
          {home && <PlaceRow c={c} l={home} ans={ans} plan={pl.outcomes[c.id]?.done ? undefined : planOn(pl.plans, c, home.volume_id)} choosing={choosing} onPicked={() => setChoosing(false)} />}
          {others.map((l) => (
            <PlaceRow key={l.id} c={c} l={l} ans={ans} plan={pl.outcomes[c.id]?.done ? undefined : planOn(pl.plans, c, l.volume_id)} choosing={choosing} onPicked={() => setChoosing(false)} />
          ))}
        </ul>
        <PlanFooter c={c} plan={planOf(pl.plans, c)} pl={pl} ans={ans} choosing={choosing} setChoosing={(v) => setChoosing(v)} />
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
              <span className="flex min-w-0 flex-wrap items-center gap-1.5 text-xs">
                <KindBadge kind={c.project_kind} />
                <span className="font-medium text-slate-100">{c.name}</span>
                <span className="text-slate-400">
                  · {nameOf(parentOf(l.root_path))} · {l.files.toLocaleString()} files · {whyOf(c)[0] ?? ""}
                </span>
              </span>
              <span className="flex flex-wrap items-center gap-2">
                <KindSelect c={c} ans={ans} />
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
    case "set_kind":
      return list.map((x) => (x.id === c.id ? { ...x, project_kind: a.project_kind } : x));
    case "same_as": {
      const other = list.find((x) => x.id === a.other_collection_id);
      if (!other || other.id === c.id) return list;
      const mergedLocs = [
        ...c.locations,
        ...other.locations.map((l) => ({ ...l, collection_id: c.id, relationship: l.relationship === "home" ? ("diverged" as const) : l.relationship })),
      ];
      return list
        .filter((x) => x.id !== other.id)
        .map((x) => (x.id === c.id ? { ...x, state: "confirmed", locations: mergedLocs } : x));
    }
    case "not_same_as": {
      const l = c.locations.find((x) => x.id === locationId);
      if (!l) return list;
      const rest = list.map((x) => (x.id === c.id ? { ...x, locations: x.locations.filter((y) => y.id !== l.id) } : x));
      return [...rest, { ...c, id: `${c.id}-${l.id}`, name: nameOf(l.root_path), state: "candidate", locations: [{ ...l, relationship: "home" }] }];
    }
    case "keep_here":
      return list.map((x) => (x.id === c.id ? { ...x, locations: x.locations.map((y) => (y.id === locationId ? { ...y, keep_here: a.keep } : y)) } : x));
    case "change_home":
      return list.map((x) =>
        x.id === c.id
          ? {
              ...x,
              locations: x.locations.map((y) =>
                y.id === locationId ? { ...y, relationship: "home" } : y.relationship === "home" ? { ...y, relationship: "diverged" } : y,
              ),
            }
          : x,
      );
    default:
      return list;
  }
}

const REUNITE_KIND_LABEL: Record<CollectionKind, string> = {
  project: "Project",
  design_set: "Design set",
  photo_subject: "Photo subject",
};

type ReuniteEntry = { plan: ReunitePlan; out?: Outcome };

function ReuniteSection({
  plans,
  settled,
  running,
  busy,
  onRun,
  onUndo,
}: {
  plans: ReunitePlan[];
  settled: Record<string, ReuniteEntry>;
  running: string | null;
  busy: boolean;
  onRun: (p: ReunitePlan) => void;
  onUndo: (key: string, batch: string) => void;
}) {
  const keyOf = (p: ReunitePlan) => `${p.collection_id}:${p.volume_id}`;
  const openKeys = new Set(plans.map(keyOf));
  const items: ReuniteEntry[] = [
    ...plans.map((plan) => ({ plan, out: settled[keyOf(plan)]?.out })),
    ...Object.entries(settled)
      .filter(([k, e]) => !openKeys.has(k) && e.out !== undefined)
      .map(([, e]) => e),
  ];
  if (items.length === 0) return null;
  return (
    <section aria-label="Put back together" className={`${card} border-l-2 border-l-emerald-500/70 p-4`}>
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h2 className="text-xs font-semibold text-emerald-300">
          Put back together ({items.length})
        </h2>
        <p className="text-xs text-slate-400">
          Files an earlier filing batch moved away from a project or set.
        </p>
      </div>
      <ul className="mt-2 divide-y divide-slate-800">
        {items.map(({ plan: p, out }) => {
          const k = keyOf(p);
          const isRunning = running === `reunite:${k}`;
          return (
            <li key={k} className="flex flex-wrap items-start justify-between gap-3 py-3 first:pt-2 last:pb-0">
              <div className="min-w-0 flex-1 space-y-1 text-xs">
                <div className="flex flex-wrap items-center gap-1.5">
                  <Chip tone="bg-emerald-500/15 text-emerald-300">{REUNITE_KIND_LABEL[p.collection_kind]}</Chip>
                  <span className="font-semibold text-slate-100">{p.collection_name}</span>
                  <span className="text-slate-300">· {p.summary}</span>
                </div>
                <p className="text-[11px] text-slate-400">
                  {p.split_batches
                    .map(
                      (b) =>
                        `Split in batch ${b.batch_id.slice(0, 8)} (${new Date(b.at).toLocaleDateString()}) · ${b.files} ${b.files === 1 ? "file" : "files"}`,
                    )
                    .join(" · ")}
                </p>
                {!out?.done && (
                  <ul className="space-y-0.5 text-[11px] text-slate-400">
                    {p.items.slice(0, 3).map((it) => (
                      <li key={it.file_id} className="flex flex-wrap items-center gap-1">
                        <span title={it.from}>{shortPath(it.from, 3)}</span>
                        <ArrowRight className="h-3 w-3 shrink-0 text-emerald-300" aria-hidden="true" />
                        <span className="text-slate-200" title={it.to}>
                          {shortPath(it.to, 3)}
                        </span>
                      </li>
                    ))}
                    {p.items.length > 3 && (
                      <li className="text-slate-500">+{p.items.length - 3} more files into {shortPath(p.target_dir, 3)}</li>
                    )}
                  </ul>
                )}
                {p.waits_for && <p className="text-[11px] text-amber-300">The plan waits: {p.waits_for}</p>}
                {out?.done && (
                  <p role="status" className="flex items-center gap-1 text-[11px] text-emerald-200">
                    <CircleCheck className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                    Put {out.moved_files.toLocaleString()} {out.moved_files === 1 ? "file" : "files"} back in {p.target_dir}.
                  </p>
                )}
                {out && !out.done && (
                  out.undone ? (
                    <p className="text-[11px] text-slate-300">Undone: files are back where they were.</p>
                  ) : (
                    <p className="text-[11px] text-rose-300">{out.reason}</p>
                  )
                )}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                {!p.waits_for && !out?.done && (
                  <button
                    type="button"
                    className={primary}
                    disabled={running !== null || busy}
                    onClick={() => onRun(p)}
                  >
                    {isRunning ? (
                      <>
                        <Loader2 className="mr-1 inline h-3.5 w-3.5 animate-spin" aria-hidden="true" /> Moving…
                      </>
                    ) : (
                      "Put back together"
                    )}
                  </button>
                )}
                {out?.done && (
                  <button type="button" className={btn} disabled={running !== null} onClick={() => onUndo(k, out.batch_id)}>
                    Undo
                  </button>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}

export function ProjectsView({ preview, onBack }: { preview: boolean; onBack: () => void }) {
  const [list, setList] = useState<Collection[] | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  const [realPlans, setRealPlans] = useState<ProjectPlan[]>([]);
  const [realReunites, setRealReunites] = useState<ReunitePlan[]>([]);
  const [settledReunites, setSettledReunites] = useState<Record<string, ReuniteEntry>>({});
  const [running, setRunning] = useState<string | null>(null);
  const [outcomes, setOutcomes] = useState<Record<string, Outcome>>({});
  const [kindFilter, setKindFilter] = useState<ProjectKind | "all">("all");

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
    // Plans come after the list: the cards show at once and gain their plan lines a moment later.
    try {
      const [plans, reunites] = await Promise.all([aegis.context.plans(), aegis.context.reunitePlans()]);
      setRealPlans(plans);
      setRealReunites(reunites);
    } catch (e) {
      setErr(errorMessage(e));
    }
  }, [preview]);

  const plans = useMemo(() => (preview ? fixturePlans(list ?? []) : realPlans), [preview, list, realPlans]);
  const reunites = useMemo(
    () =>
      preview
        ? fixtureReunitePlans.filter((p) => !settledReunites[`${p.collection_id}:${p.volume_id}`]?.out?.done)
        : realReunites,
    [preview, realReunites, settledReunites],
  );

  const runReunite = (p: ReunitePlan) => {
    const k = `${p.collection_id}:${p.volume_id}`;
    if (running) return;
    setRunning(`reunite:${k}`);
    setErr(null);
    void (async () => {
      try {
        const out: PlanOutcome = preview
          ? await new Promise((r) =>
              window.setTimeout(
                () => r({ batch_id: `preview-reunite-${k}`, done: true, reason: null, moved_files: p.items.length, freed_bytes: 0 }),
                600,
              ),
            )
          : await aegis.context.runReunite(p.collection_id, p.volume_id, p.digest);
        setSettledReunites((s) => ({ ...s, [k]: { plan: p, out } }));
        if (!preview && out.done) await load();
      } catch (e) {
        setErr(errorMessage(e));
      } finally {
        setRunning(null);
      }
    })();
  };

  const undoReunite = (k: string, batch: string) => {
    if (running) return;
    setRunning(`reunite:${k}`);
    void (async () => {
      try {
        if (!preview) {
          await aegis.filing.undo(batch);
          await load();
        }
        setSettledReunites((s) =>
          s[k]?.out ? { ...s, [k]: { ...s[k], out: { ...s[k].out!, done: false, undone: true } } } : s,
        );
      } catch (e) {
        setErr(errorMessage(e));
      } finally {
        setRunning(null);
      }
    })();
  };

  const pl: Planning = {
    plans,
    running,
    outcomes,
    approve: (c, p) => {
      if (running) return;
      setRunning(c.id);
      setErr(null);
      void (async () => {
        try {
          const out: PlanOutcome = preview
            ? await new Promise((r) =>
                window.setTimeout(
                  () => r({ batch_id: `preview-${c.id}`, done: true, reason: null, moved_files: c.locations.reduce((n, l) => n + l.files, 0), freed_bytes: 25_000_000 }),
                  900,
                ),
              )
            : await aegis.context.runPlan(c.id, p.volume_id, p.digest);
          setOutcomes((o) => ({ ...o, [c.id]: out }));
          if (!preview && out.done) await load();
        } catch (e) {
          setErr(errorMessage(e));
        } finally {
          setRunning(null);
        }
      })();
    },
    undo: (c, batch) => {
      if (running) return;
      setRunning(c.id);
      void (async () => {
        try {
          if (!preview) {
            await aegis.filing.undo(batch);
            await load();
          }
          setOutcomes((o) => ({ ...o, [c.id]: { ...o[c.id], done: false, undone: true } }));
        } catch (e) {
          setErr(errorMessage(e));
        } finally {
          setRunning(null);
        }
      })();
    },
  };

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
    busy: busy || running !== null,
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
      const parts = [`${rep.projects.toLocaleString()} projects`];
      if (rep.design_sets > 0) parts.push(`${rep.design_sets.toLocaleString()} design sets`);
      if (rep.photo_subjects > 0) parts.push(`${rep.photo_subjects.toLocaleString()} photo subjects`);
      setNote(`Found ${parts.join(", ")} in ${(rep.took_ms / 1000).toFixed(0)} s.`);
    });

  const kindCounts = useMemo(() => {
    const counts: Record<ProjectKind, number> = { software: 0, design: 0, writing: 0, research: 0, other: 0 };
    for (const c of list ?? []) {
      const k = c.project_kind ?? "other";
      counts[k] = (counts[k] ?? 0) + 1;
    }
    return counts;
  }, [list]);

  const groups = useMemo(() => {
    const all = (list ?? []).filter((c) => kindFilter === "all" || (c.project_kind ?? "other") === kindFilter);
    const asks = (c: Collection) => c.state === "candidate" && c.confidence < SURE && c.locations.length === 1;
    // Simplest and safest plans first (spec §8.5), then the rest.
    const rank = (c: Collection) => planOf(plans, c)?.rank ?? 9;
    const byRank = (a: Collection, b: Collection) => rank(a) - rank(b);
    return {
      multi: all.filter((c) => c.locations.length > 1).sort(byRank),
      confirm: all.filter(asks),
      single: all.filter((c) => c.locations.length === 1 && !asks(c)).sort(byRank),
    };
  }, [list, plans, kindFilter]);

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
      {list && list.length > 0 && (
        <div role="tablist" aria-label="Filter projects by kind" className="flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            role="tab"
            aria-selected={kindFilter === "all"}
            onClick={() => setKindFilter("all")}
            className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium transition-colors ${
              kindFilter === "all"
                ? "border-emerald-500/60 bg-emerald-500/15 text-emerald-200"
                : "border-slate-700 text-slate-300 hover:bg-slate-800"
            }`}
          >
            All <span className="text-[11px] opacity-80">{list.length}</span>
          </button>
          {KIND_ORDER.filter((k) => kindCounts[k] > 0 || kindFilter === k).map((k) => {
            const m = KIND_META[k];
            const active = kindFilter === k;
            return (
              <button
                key={k}
                type="button"
                role="tab"
                aria-selected={active}
                onClick={() => setKindFilter(k)}
                className={`inline-flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium transition-colors ${
                  active ? m.pill : "border-slate-700 text-slate-300 hover:bg-slate-800"
                }`}
              >
                {m.icon}
                {m.label}
                <span className="text-[11px] opacity-80">{kindCounts[k]}</span>
              </button>
            );
          })}
        </div>
      )}
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
      <ReuniteSection
        plans={reunites}
        settled={settledReunites}
        running={running}
        busy={busy}
        onRun={runReunite}
        onUndo={undoReunite}
      />
      <ConfirmList items={groups.confirm} ans={ans} />
      {groups.multi.map((c) => (
        <ProjectCard
          key={c.id}
          c={c}
          otherProjects={(list ?? []).filter((x) => x.kind === "project" && x.id !== c.id)}
          ans={ans}
          pl={pl}
        />
      ))}
      {groups.single.length > 0 && (
        <section aria-label="Projects in one place" className={`${card} p-4`}>
          <h2 className="text-xs font-semibold text-slate-300">In one place ({groups.single.length})</h2>
          <ul className="mt-2">
            {groups.single.map((c) => {
              const p = planOf(plans, c);
              const move = p && !p.waits_for && c.state !== "left_as_is" ? p.steps.find((s) => s.kind === "move_home" || s.kind === "gather_files") : undefined;
              const out = outcomes[c.id];
              return (
                <li
                  key={c.id}
                  className={`flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 py-2 last:border-b-0 ${
                    c.project_kind === "design" ? "border-l-2 border-l-fuchsia-500/60 pl-2" : ""
                  }`}
                >
                  <span className="min-w-0 text-xs">
                    <span className="inline-flex flex-wrap items-center gap-1.5">
                      <KindBadge kind={c.project_kind} />
                      <span className="font-medium text-slate-100">{c.name}</span>
                      <span className="text-slate-400" title={c.locations[0].root_path}>
                        · {shortPath(c.locations[0].root_path, 3)}
                      </span>
                      <SureChip c={c} />
                    </span>
                    {move && !out?.done && (
                      <span className="mt-0.5 flex items-center gap-1 text-[11px] text-slate-200" title={move.reason}>
                        <ArrowRight className="h-3 w-3 shrink-0 text-emerald-300" aria-hidden="true" />{" "}
                        {move.kind === "gather_files" ? `moves matching files into ${move.to}` : `moves whole to ${move.to}`}
                      </span>
                    )}
                    {out?.done && (
                      <span role="status" className="mt-0.5 flex items-center gap-1 text-[11px] text-emerald-200">
                        <CircleCheck className="h-3 w-3" aria-hidden="true" /> Moved to {p?.home_after ?? "your projects home"}
                      </span>
                    )}
                    {out && !out.done && (out.undone ? <span className="mt-0.5 block text-[11px] text-slate-300">Undone: it is back where it was.</span> : <span className="mt-0.5 block text-[11px] text-rose-300">{out.reason}</span>)}
                  </span>
                  <span className="flex flex-wrap items-center gap-2">
                    <KindSelect c={c} ans={ans} />
                    {move && p && !out?.done && (
                      <button type="button" className={primary} disabled={running !== null || busy} onClick={() => pl.approve(c, p)}>
                        {running === c.id ? (
                          <>
                            <Loader2 className="mr-1 inline h-3.5 w-3.5 animate-spin" aria-hidden="true" /> Moving…
                          </>
                        ) : (
                          "Move"
                        )}
                      </button>
                    )}
                    {out?.done && (
                      <button type="button" className={btn} disabled={running !== null} onClick={() => pl.undo(c, out.batch_id)}>
                        Undo
                      </button>
                    )}
                    <button type="button" className={btn} disabled={ans.busy} onClick={() => void ans.send(c, { kind: "is_project", yes: false })}>
                      Not a project
                    </button>
                  </span>
                </li>
              );
            })}
          </ul>
        </section>
      )}
    </div>
  );
}
