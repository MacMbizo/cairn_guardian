import { useEffect, useMemo, useState } from "react";
import { ArrowRight, Archive, Check, File, Lock, RotateCcw, ShieldCheck, X } from "lucide-react";
import { aegis } from "../bindings/client";
import type { ActionItem, ActionStatus, ActionType, Simulation } from "../bindings";
import { Badge, Button, type Tone, ui } from "../ui";

const ACTION_LABEL: Record<ActionType, string> = {
  move: "Move",
  copy: "Copy",
  rename: "Rename",
  quarantine: "To quarantine",
  purge: "Delete for good",
  archive: "Archive",
  restore: "Restore",
  patch_text: "Edit text",
};

const STATUS: Partial<Record<ActionStatus, { label: string; tone: Tone }>> = {
  approved: { label: "Approved", tone: "info" },
  executed: { label: "Done", tone: "success" },
  failed: { label: "Failed", tone: "danger" },
  skipped: { label: "Skipped", tone: "neutral" },
  rolled_back: { label: "Undone", tone: "neutral" },
  rejected: { label: "Rejected", tone: "neutral" },
  superseded: { label: "Replaced", tone: "neutral" },
  expired: { label: "Expired", tone: "neutral" },
  unrecoverable_missing: { label: "Missing", tone: "warning" },
};

const PAGE = 150;

function split(path: string): [string, string] {
  const i = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return i < 0 ? ["", path] : [path.slice(0, i), path.slice(i + 1)];
}

function PathCell({ path, muted = false }: { path: string; muted?: boolean }) {
  const [dir, name] = split(path);
  return (
    <div className="min-w-0" title={path}>
      <div className={`truncate font-mono text-sm ${muted ? "text-slate-400" : "text-slate-100"}`}>{name}</div>
      {dir && <div className="truncate font-mono text-xs text-slate-500">{dir}</div>}
    </div>
  );
}

function confidenceTone(c: number): Tone {
  return c >= 0.9 ? "success" : c >= 0.6 ? "warning" : "neutral";
}

/**
 * D1 split-pane Simulator for one batch: what each file is now, and where it will be after.
 * One primary action approves and applies the ticked changes; every applied batch can be undone.
 */
export function SimulatorBatch({
  batchId,
  items,
  desktop,
  busy,
  onApply,
  onReject,
  onUndo,
}: {
  batchId: string;
  items: ActionItem[];
  desktop: boolean;
  busy: boolean;
  onApply: (batchId: string, ids: string[]) => Promise<void> | void;
  onReject: (batchId: string) => void;
  onUndo: (batchId: string) => void;
}) {
  const pending = items.filter((a) => a.status === "proposed" || a.status === "approved");
  const executed = items.filter((a) => a.status === "executed").length;
  const [sim, setSim] = useState<Simulation | null>(null);
  const [picked, setPicked] = useState<Set<string> | null>(null);
  const [shown, setShown] = useState(PAGE);

  // Auto-simulate (read-only dry run) whenever this batch has changes waiting.
  const pendingKey = pending.map((a) => a.id).join(",");
  useEffect(() => {
    if (!desktop || !pendingKey) return;
    let cancelled = false;
    aegis.actions
      .simulate(batchId)
      .then((s) => !cancelled && setSim(s))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [desktop, batchId, pendingKey]);

  const simById = useMemo(() => new Map((sim?.items ?? []).map((i) => [i.action_id, i])), [sim]);
  const allowed = (a: ActionItem) => {
    const s = simById.get(a.id);
    return !s || (s.policy_allowed && !s.atomic_unit_protected);
  };
  // Default selection: changes the policy allows that Cairn is sure about (90%+). Anything less
  // certain starts unticked, so applying it is always a deliberate choice (D1 "review needed").
  const sure = (a: ActionItem) => (simById.get(a.id)?.confidence ?? a.confidence) >= 0.9;
  const selection = picked ?? new Set(pending.filter((a) => allowed(a) && sure(a)).map((a) => a.id));
  const toggle = (id: string, on: boolean) => {
    const next = new Set(selection);
    if (on) next.add(id);
    else next.delete(id);
    setPicked(next);
  };

  const ready = sim?.ready_count ?? 0;
  const review = sim?.review_count ?? 0;
  const blocked = sim?.blocked_count ?? 0;
  const applyIds = pending.filter((a) => selection.has(a.id)).map((a) => a.id);

  return (
    <section className={ui.card} aria-labelledby={`batch-${batchId}`}>
      <div className="mb-3 flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <h2 id={`batch-${batchId}`} className={ui.sectionTitle}>
            {items[0]?.reason ? items[0].reason : "Changes"}{" "}
            <span className="font-mono text-xs font-normal text-slate-500">#{batchId.slice(0, 6)}</span>
          </h2>
          <div className="mt-1.5 flex flex-wrap gap-1.5">
            <Badge>{items.length} change{items.length === 1 ? "" : "s"}</Badge>
            {sim && ready > 0 && <Badge tone="success" icon={Check}>{ready} ready</Badge>}
            {sim && review > 0 && <Badge tone="warning">{review} to check</Badge>}
            {sim && blocked > 0 && <Badge tone="danger" icon={Lock}>{blocked} kept whole</Badge>}
            {executed > 0 && <Badge tone="success">{executed} done</Badge>}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {pending.length > 0 && (
            <Button variant="ghost" icon={X} disabled={busy || !desktop} onClick={() => onReject(batchId)}>
              Reject
            </Button>
          )}
          {executed > 0 && (
            <Button icon={RotateCcw} disabled={busy || !desktop} onClick={() => onUndo(batchId)}>
              Undo batch
            </Button>
          )}
          {pending.length > 0 && (
            <Button
              variant="primary"
              icon={ShieldCheck}
              loading={busy}
              disabled={!desktop || applyIds.length === 0}
              onClick={() => void onApply(batchId, applyIds)}
            >
              Approve &amp; apply {applyIds.length} change{applyIds.length === 1 ? "" : "s"}
            </Button>
          )}
        </div>
      </div>
      {pending.length > 0 && (
        <p className="mb-3 text-xs text-slate-400">
          Nothing has changed yet. Files sent to quarantine can be restored for 30 days, and the whole batch can be undone.
        </p>
      )}

      <div className="overflow-hidden rounded-lg border border-slate-800">
        <div className="grid grid-cols-[1.75rem_minmax(0,1fr)_1.25rem_minmax(0,1fr)] items-center gap-3 border-b border-slate-800 bg-slate-950/60 px-3 py-2 text-xs font-semibold uppercase tracking-wider text-slate-500">
          <span />
          <span>Now</span>
          <span />
          <span>After</span>
        </div>
        <ul className="divide-y divide-slate-800/80">
          {items.slice(0, shown).map((a) => {
            const s = simById.get(a.id);
            const isPending = a.status === "proposed" || a.status === "approved";
            const ok = allowed(a);
            const to = s?.effective_to_path ?? a.to_path;
            const status = STATUS[a.status];
            return (
              <li key={a.id} className="grid grid-cols-[1.75rem_minmax(0,1fr)_1.25rem_minmax(0,1fr)] items-center gap-3 px-3 py-2.5">
                <span>
                  {isPending && (
                    <input
                      type="checkbox"
                      aria-label={`Include ${split(a.from_path)[1]}`}
                      disabled={!ok}
                      checked={selection.has(a.id)}
                      onChange={(e) => toggle(a.id, e.target.checked)}
                      className="h-4 w-4 rounded border-slate-600 bg-slate-900 accent-emerald-500"
                    />
                  )}
                </span>
                <div className="flex min-w-0 items-start gap-2">
                  <File className="mt-0.5 h-4 w-4 shrink-0 text-slate-500" aria-hidden="true" />
                  <PathCell path={a.from_path} />
                </div>
                <ArrowRight className="h-4 w-4 text-slate-600" aria-hidden="true" />
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
                  {!ok && s?.atomic_unit_protected ? (
                    <div className="flex items-center gap-1.5 rounded-md border border-rose-900/70 bg-rose-950/40 px-2 py-1 text-xs text-rose-200">
                      <Lock className="h-3.5 w-3.5" aria-hidden="true" /> Kept whole: it is inside a project or repo
                    </div>
                  ) : a.action_type === "quarantine" ? (
                    <div className="flex items-center gap-1.5 text-sm text-amber-200">
                      <Archive className="h-4 w-4" aria-hidden="true" /> Quarantine (restorable for 30 days)
                    </div>
                  ) : to ? (
                    <PathCell path={to} />
                  ) : (
                    <span className="text-sm text-slate-400">{ACTION_LABEL[a.action_type]}</span>
                  )}
                  <div className="flex shrink-0 flex-wrap items-center gap-1.5">
                    {s?.collision_renamed && <Badge tone="info">Renamed to avoid overwriting</Badge>}
                    {isPending && <Badge tone={confidenceTone(s?.confidence ?? a.confidence)}>{Math.round((s?.confidence ?? a.confidence) * 100)}% sure</Badge>}
                    {!isPending && status && <Badge tone={status.tone}>{status.label}</Badge>}
                  </div>
                </div>
                {a.error_message && <p className="col-span-4 text-xs text-rose-300">{a.error_message}</p>}
              </li>
            );
          })}
        </ul>
      </div>
      {items.length > shown && (
        <div className="mt-3 flex justify-center">
          <Button variant="ghost" onClick={() => setShown((n) => n + PAGE * 4)}>
            Show more ({items.length - shown} not shown)
          </Button>
        </div>
      )}
    </section>
  );
}
