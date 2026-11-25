import { useEffect, useRef } from "react";
import { AlertTriangle, ShieldCheck } from "lucide-react";
import type { ConflictResolutionChoice, RollbackConflictItem } from "../bindings";
import { Badge, Button } from "../ui";

type Option = { value: ConflictResolutionChoice; title: string; detail: string; danger?: boolean };

const STOP: Option = { value: "abort_batch", title: "Stop the whole undo", detail: "Nothing else in this batch is undone.", danger: true };

function optionsFor(c: RollbackConflictItem): Option[] {
  if (c.conflict === "modified")
    return [
      { value: "copy_back_keeping_both", title: "Put a copy back and keep both", detail: "Your edited file stays where it is, and a copy returns to the original place." },
      { value: "revert_keeping_edits", title: "Move the edited file back", detail: "The file, with your edits, returns to where it was before." },
      { value: "skip", title: "Leave this file where it is", detail: "Only this file is skipped; the rest of the batch is still undone." },
      STOP,
    ];
  if (c.conflict === "collision")
    return [
      { value: "restore_as_rename", title: "Restore under a new name", detail: "Something else now sits in the original place, so the file comes back beside it with “restored” in its name." },
      { value: "skip", title: "Leave this file where it is", detail: "Only this file is skipped; the rest of the batch is still undone." },
      STOP,
    ];
  return [
    { value: "skip", title: "Skip it: the file is gone", detail: "It was deleted or moved outside Cairn, so there is nothing to bring back." },
    STOP,
  ];
}

const KIND: Record<string, string> = {
  modified: "Edited since it was moved",
  collision: "Its old place is taken",
  missing_target: "No longer there",
};

/**
 * D1 pre-rollback conflict resolver: explains that nothing has changed yet, and offers each file's
 * choices as keyboard-navigable option cards with the safe, non-destructive choice preselected.
 */
export function ConflictResolver({
  conflicts,
  choices,
  onChoose,
  onApply,
  onClose,
  busy,
}: {
  conflicts: RollbackConflictItem[];
  choices: Record<string, ConflictResolutionChoice>;
  onChoose: (actionId: string, choice: ConflictResolutionChoice) => void;
  onApply: (actionId: string) => void;
  onClose: () => void;
  busy: boolean;
}) {
  const firstRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (conflicts.length) requestAnimationFrame(() => firstRef.current?.focus());
  }, [conflicts.length]);
  if (!conflicts.length) return null;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/80 p-4">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="conflict-title"
        className="flex max-h-[88vh] w-full max-w-2xl flex-col overflow-hidden rounded-xl border border-slate-700 bg-slate-900 shadow-2xl shadow-black/50"
      >
        <div className="border-b border-slate-800 p-5">
          <h2 id="conflict-title" className="flex items-center gap-2 text-base font-semibold text-slate-50">
            <AlertTriangle className="h-5 w-5 text-amber-400" aria-hidden="true" />
            {conflicts.length === 1 ? "One file changed after this batch" : `${conflicts.length} files changed after this batch`}
          </h2>
          <div className="mt-3 flex items-start gap-2 rounded-lg border border-emerald-900/70 bg-emerald-950/30 px-3 py-2 text-sm text-emerald-100">
            <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-emerald-400" aria-hidden="true" />
            <span>
              <strong>Nothing has been changed yet.</strong> Choose what to do with each file below. Once every file has a decision, the rest of the batch is undone.
            </span>
          </div>
        </div>

        <div className="flex-1 space-y-4 overflow-y-auto p-5">
          {conflicts.map((c, ci) => {
            const current = choices[c.action_id] ?? c.recommended_choice;
            const name = c.from_path.split(/[\\/]/).pop();
            return (
              <fieldset key={c.action_id} className="rounded-lg border border-slate-800 bg-slate-950/50 p-3.5">
                <legend className="sr-only">{`What to do with ${name}`}</legend>
                <div className="mb-2.5 flex flex-wrap items-center justify-between gap-2">
                  <div className="min-w-0">
                    <div className="truncate font-mono text-sm text-slate-100" title={c.from_path}>{name}</div>
                    <div className="truncate font-mono text-xs text-slate-500">{c.from_path}</div>
                  </div>
                  <Badge tone="warning">{KIND[c.conflict] ?? c.conflict}</Badge>
                </div>
                <div className="grid gap-2">
                  {optionsFor(c).map((o, oi) => {
                    const checked = current === o.value;
                    return (
                      <label
                        key={o.value}
                        className={`flex cursor-pointer items-start gap-3 rounded-lg border px-3 py-2.5 transition-colors focus-within:ring-2 focus-within:ring-emerald-400 ${
                          checked
                            ? o.danger
                              ? "border-rose-700 bg-rose-950/40"
                              : "border-emerald-700 bg-emerald-950/30"
                            : "border-slate-800 hover:border-slate-700"
                        }`}
                      >
                        <input
                          ref={ci === 0 && oi === 0 ? firstRef : undefined}
                          type="radio"
                          name={`conflict-${c.action_id}`}
                          value={o.value}
                          checked={checked}
                          onChange={() => onChoose(c.action_id, o.value)}
                          className="mt-1 h-4 w-4 accent-emerald-500"
                        />
                        <span className="min-w-0">
                          <span className={`flex flex-wrap items-center gap-2 text-sm font-medium ${o.danger ? "text-rose-200" : "text-slate-100"}`}>
                            {o.title}
                            {o.value === c.recommended_choice && <Badge tone="success">Recommended</Badge>}
                          </span>
                          <span className="mt-0.5 block text-xs text-slate-400">{o.detail}</span>
                        </span>
                      </label>
                    );
                  })}
                </div>
                <div className="mt-3 flex justify-end">
                  <Button variant={current === "abort_batch" ? "danger" : "primary"} loading={busy} onClick={() => onApply(c.action_id)}>
                    {current === "abort_batch" ? "Stop the undo" : "Use this choice"}
                  </Button>
                </div>
              </fieldset>
            );
          })}
        </div>

        <div className="flex items-center justify-between gap-3 border-t border-slate-800 px-5 py-3 text-xs text-slate-400">
          <span>You can close this and decide later; nothing changes until you choose.</span>
          <Button variant="ghost" onClick={onClose}>
            Decide later <kbd className="ml-1 font-mono text-slate-500">Esc</kbd>
          </Button>
        </div>
      </div>
    </div>
  );
}
