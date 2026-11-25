import { useEffect, useMemo, useRef, useState, type ComponentType } from "react";
import { CornerDownLeft, Search } from "lucide-react";

export type Command = {
  id: string;
  label: string;
  group: string;
  icon: ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;
  /** Extra words that should find this command. */
  keywords?: string;
  run: () => void;
};

/**
 * D1 command bar (Ctrl+K): jump to any screen, run a common action, or search files.
 * Keyboard: type to filter, Up/Down to move, Enter to run, Esc to close (focus returns to where it was).
 */
export function CommandPalette({
  open,
  onClose,
  commands,
  onSearchFiles,
}: {
  open: boolean;
  onClose: () => void;
  commands: Command[];
  onSearchFiles: (query: string) => void;
}) {
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const returnFocus = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (open) {
      returnFocus.current = document.activeElement as HTMLElement | null;
      setQ("");
      setActive(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    } else {
      returnFocus.current?.focus?.();
    }
  }, [open]);

  const items = useMemo(() => {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    const matched = words.length
      ? commands.filter((c) => {
          const hay = `${c.label} ${c.group} ${c.keywords ?? ""}`.toLowerCase();
          return words.every((w) => hay.includes(w));
        })
      : commands;
    const search: Command[] = q.trim()
      ? [{ id: "search-files", label: `Search files for “${q.trim()}”`, group: "Search", icon: Search, run: () => onSearchFiles(q.trim()) }]
      : [];
    return [...search, ...matched];
  }, [q, commands, onSearchFiles]);

  useEffect(() => setActive(0), [q]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  if (!open) return null;

  const runItem = (c: Command | undefined) => {
    if (!c) return;
    onClose();
    c.run();
  };

  let lastGroup = "";
  return (
    <div className="fixed inset-0 z-50 flex items-start justify-center bg-slate-950/75 px-4 pt-[12vh]" onMouseDown={onClose}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command bar"
        className="w-full max-w-xl overflow-hidden rounded-xl border border-slate-700 bg-slate-900 shadow-2xl shadow-black/50"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") { e.preventDefault(); onClose(); }
          else if (e.key === "ArrowDown") { e.preventDefault(); setActive((i) => Math.min(items.length - 1, i + 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setActive((i) => Math.max(0, i - 1)); }
          else if (e.key === "Enter") { e.preventDefault(); runItem(items[active]); }
          else if (e.key === "Tab") e.preventDefault(); // focus stays in the bar
        }}
      >
        <div className="flex items-center gap-2.5 border-b border-slate-800 px-4">
          <Search className="h-4 w-4 shrink-0 text-slate-500" aria-hidden="true" />
          <input
            ref={inputRef}
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder="Go to a screen, run an action, or search your files…"
            role="combobox"
            aria-expanded="true"
            aria-controls="command-list"
            aria-activedescendant={items[active] ? `cmd-${items[active].id}` : undefined}
            aria-label="Command or search"
            className="h-12 flex-1 bg-transparent text-sm text-slate-100 placeholder:text-slate-500 focus:outline-none"
          />
          <kbd className="rounded border border-slate-700 px-1.5 font-mono text-xs text-slate-500">Esc</kbd>
        </div>
        <ul id="command-list" role="listbox" ref={listRef} className="max-h-[50vh] overflow-y-auto py-1.5">
          {items.length === 0 && <li className="px-4 py-6 text-center text-sm text-slate-400">Nothing matches. Try other words.</li>}
          {items.map((c, i) => {
            const header = c.group !== lastGroup ? c.group : null;
            lastGroup = c.group;
            const Icon = c.icon;
            const selected = i === active;
            return (
              <li key={c.id} role="presentation">
                {header && <div className="px-4 pb-1 pt-2 text-xs font-semibold uppercase tracking-wider text-slate-500">{header}</div>}
                <div
                  id={`cmd-${c.id}`}
                  data-index={i}
                  role="option"
                  aria-selected={selected}
                  onMouseMove={() => setActive(i)}
                  onClick={() => runItem(c)}
                  className={`mx-1.5 flex cursor-pointer items-center gap-3 rounded-md px-2.5 py-2 text-sm ${
                    selected ? "bg-emerald-500/15 text-emerald-200" : "text-slate-200"
                  }`}
                >
                  <Icon className="h-4 w-4 shrink-0 opacity-80" aria-hidden="true" />
                  <span className="flex-1 truncate">{c.label}</span>
                  {selected && <CornerDownLeft className="h-3.5 w-3.5 opacity-70" aria-hidden="true" />}
                </div>
              </li>
            );
          })}
        </ul>
        <div className="flex items-center gap-4 border-t border-slate-800 px-4 py-2 text-xs text-slate-500">
          <span><kbd className="font-mono">↑ ↓</kbd> move</span>
          <span><kbd className="font-mono">Enter</kbd> open</span>
          <span><kbd className="font-mono">Esc</kbd> close</span>
        </div>
      </div>
    </div>
  );
}
