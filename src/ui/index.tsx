/**
 * Cairn's shared interface kit (D1 "flat precision", dark).
 *
 * Every screen builds its header, cards, buttons, badges and notices from here, so the app looks
 * and behaves the same everywhere. Change a look once, here, rather than per screen.
 */
import type { ComponentType, ReactNode, ButtonHTMLAttributes } from "react";
import { Loader2 } from "lucide-react";

type IconType = ComponentType<{ className?: string; "aria-hidden"?: boolean | "true" | "false" }>;

const focus = "focus:outline-none focus-visible:ring-2 focus-visible:ring-emerald-400 focus-visible:ring-offset-2 focus-visible:ring-offset-slate-950";

/** Class strings for places that can't use the components (e.g. <a>, <label>, <select>). */
export const ui = {
  /** Raised panel. */
  card: "rounded-xl border border-slate-800 bg-slate-900/60 p-4",
  /** Panel nested inside a card. */
  inset: "rounded-lg border border-slate-800 bg-slate-950/60 p-3",
  /** Secondary button. */
  btn: `inline-flex items-center justify-center gap-1.5 rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium text-slate-200 transition-colors hover:bg-slate-800 hover:text-slate-50 disabled:cursor-not-allowed disabled:opacity-45 ${focus}`,
  /** The one primary action on a screen or card. */
  primary: `inline-flex items-center justify-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white transition-colors hover:bg-emerald-500 disabled:cursor-not-allowed disabled:opacity-45 ${focus}`,
  /** Quiet button for low-emphasis actions. */
  ghost: `inline-flex items-center justify-center gap-1.5 rounded-md px-2.5 py-1.5 text-xs font-medium text-slate-300 transition-colors hover:bg-slate-800 hover:text-slate-50 disabled:cursor-not-allowed disabled:opacity-45 ${focus}`,
  /** Destructive action: always paired with a confirmation. */
  danger: `inline-flex items-center justify-center gap-1.5 rounded-md border border-rose-800 bg-rose-950/50 px-3 py-1.5 text-xs font-semibold text-rose-200 transition-colors hover:bg-rose-900/60 disabled:cursor-not-allowed disabled:opacity-45 ${focus}`,
  /** Text input / select / textarea. */
  input: `rounded-md border border-slate-700 bg-slate-950 px-3 py-1.5 text-sm text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 ${focus}`,
  /** Small uppercase label above a group. */
  eyebrow: "text-xs font-semibold uppercase tracking-wider text-slate-500",
  /** Card / section title. */
  sectionTitle: "text-sm font-semibold text-slate-100",
  /** Secondary text. */
  muted: "text-xs text-slate-400",
  focus,
} as const;

/* ------------------------------------------------------------------ Buttons */

type Variant = "primary" | "secondary" | "ghost" | "danger";

export function Button({
  variant = "secondary",
  icon: Icon,
  loading = false,
  children,
  className = "",
  type = "button",
  disabled,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  icon?: IconType;
  /** Shows a spinner and blocks repeat clicks while a job runs (single-flight). */
  loading?: boolean;
}) {
  const base = variant === "primary" ? ui.primary : variant === "ghost" ? ui.ghost : variant === "danger" ? ui.danger : ui.btn;
  return (
    <button
      type={type}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      className={`${base} ${className}`}
      {...rest}
    >
      {loading ? (
        <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden="true" />
      ) : (
        Icon && <Icon className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
      )}
      {children}
    </button>
  );
}

/* ------------------------------------------------------------------ Layout */

/** Top of every screen: icon, title, one-line purpose, optional badges and actions. */
export function PageHeader({
  icon: Icon,
  title,
  description,
  badges,
  actions,
}: {
  icon?: IconType;
  title: ReactNode;
  description?: ReactNode;
  badges?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <header className="flex flex-wrap items-start justify-between gap-4">
      <div className="min-w-0 flex-1">
        <div className="flex flex-wrap items-center gap-2.5">
          {Icon && (
            <span className="flex h-8 w-8 shrink-0 items-center justify-center rounded-lg border border-slate-800 bg-slate-900 text-emerald-400">
              <Icon className="h-4 w-4" aria-hidden="true" />
            </span>
          )}
          <h1 className="text-lg font-semibold text-slate-50">{title}</h1>
          {badges}
        </div>
        {description && <p className="mt-1.5 max-w-3xl text-sm leading-relaxed text-slate-400">{description}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </header>
  );
}

/** A titled panel. Use `actions` for the card's own buttons (top right). */
export function Card({
  title,
  icon: Icon,
  description,
  actions,
  children,
  className = "",
  bodyClassName = "",
}: {
  title?: ReactNode;
  icon?: IconType;
  description?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  className?: string;
  bodyClassName?: string;
}) {
  return (
    <section className={`${ui.card} ${className}`}>
      {(title || actions) && (
        <div className="mb-3 flex flex-wrap items-start justify-between gap-2">
          <div className="min-w-0">
            {title && (
              <h2 className={`flex items-center gap-2 ${ui.sectionTitle}`}>
                {Icon && <Icon className="h-4 w-4 shrink-0 text-emerald-400" aria-hidden="true" />}
                {title}
              </h2>
            )}
            {description && <p className="mt-0.5 text-xs text-slate-400">{description}</p>}
          </div>
          {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
        </div>
      )}
      <div className={bodyClassName}>{children}</div>
    </section>
  );
}

/** A single number with its label, for KPI rows. */
export function Stat({ label, value, hint, tone = "neutral" }: { label: ReactNode; value: ReactNode; hint?: ReactNode; tone?: Tone }) {
  return (
    <div className={ui.card}>
      <div className="text-xs font-medium uppercase tracking-wide text-slate-400">{label}</div>
      <div className={`mt-1 font-mono text-2xl ${TONE_TEXT[tone]}`}>{value}</div>
      {hint && <div className="mt-1 text-xs text-slate-400">{hint}</div>}
    </div>
  );
}

/* ------------------------------------------------------------------ Status */

export type Tone = "neutral" | "success" | "warning" | "danger" | "info";

const TONE_TEXT: Record<Tone, string> = {
  neutral: "text-slate-100",
  success: "text-emerald-300",
  warning: "text-amber-300",
  danger: "text-rose-300",
  info: "text-sky-300",
};

const TONE_BADGE: Record<Tone, string> = {
  neutral: "border-slate-700 bg-slate-800/70 text-slate-300",
  success: "border-emerald-800/70 bg-emerald-950/50 text-emerald-200",
  warning: "border-amber-800/70 bg-amber-950/50 text-amber-200",
  danger: "border-rose-800/70 bg-rose-950/50 text-rose-200",
  info: "border-sky-800/70 bg-sky-950/50 text-sky-200",
};

/** Status label. Never colour alone: always text, optionally an icon (D1). */
export function Badge({ tone = "neutral", icon: Icon, children, className = "" }: { tone?: Tone; icon?: IconType; children: ReactNode; className?: string }) {
  return (
    <span className={`inline-flex items-center gap-1 whitespace-nowrap rounded-full border px-2 py-0.5 text-xs font-medium ${TONE_BADGE[tone]} ${className}`}>
      {Icon && <Icon className="h-3 w-3 shrink-0" aria-hidden="true" />}
      {children}
    </span>
  );
}

const TONE_NOTICE: Record<Tone, string> = {
  neutral: "border-slate-800 bg-slate-900/60 text-slate-200",
  success: "border-emerald-800/60 bg-emerald-950/30 text-emerald-100",
  warning: "border-amber-800/60 bg-amber-950/30 text-amber-100",
  danger: "border-rose-800/60 bg-rose-950/40 text-rose-100",
  info: "border-sky-800/60 bg-sky-950/30 text-sky-100",
};

/** Inline message. Errors are announced to screen readers; others politely. */
export function Notice({
  tone = "neutral",
  icon: Icon,
  title,
  children,
  actions,
}: {
  tone?: Tone;
  icon?: IconType;
  title?: ReactNode;
  children?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div
      role={tone === "danger" ? "alert" : "status"}
      className={`flex flex-wrap items-start gap-3 rounded-lg border px-3.5 py-2.5 text-sm ${TONE_NOTICE[tone]}`}
    >
      {Icon && <Icon className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />}
      <div className="min-w-0 flex-1">
        {title && <p className="font-semibold">{title}</p>}
        {children && <div className={title ? "mt-0.5 text-xs opacity-90" : "text-xs"}>{children}</div>}
      </div>
      {actions && <div className="flex items-center gap-2">{actions}</div>}
    </div>
  );
}

/** What to show when a list is empty: what this is, and what to do next. */
export function EmptyState({ icon: Icon, title, children, action }: { icon?: IconType; title: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center rounded-xl border border-dashed border-slate-800 px-6 py-10 text-center">
      {Icon && <Icon className="mb-3 h-6 w-6 text-slate-500" aria-hidden="true" />}
      <p className="text-sm font-medium text-slate-200">{title}</p>
      {children && <p className="mt-1 max-w-md text-xs text-slate-400">{children}</p>}
      {action && <div className="mt-4">{action}</div>}
    </div>
  );
}

/* ------------------------------------------------------------------ Tabs */

export type TabDef<T extends string> = { id: T; label: ReactNode; icon?: IconType; count?: number };

/** Sub-navigation inside a screen. Arrow keys move between tabs (WAI-ARIA tabs pattern). */
export function Tabs<T extends string>({ tabs, value, onChange, label }: { tabs: TabDef<T>[]; value: T; onChange: (id: T) => void; label: string }) {
  const move = (dir: 1 | -1) => {
    const i = tabs.findIndex((t) => t.id === value);
    const next = tabs[(i + dir + tabs.length) % tabs.length];
    onChange(next.id);
    requestAnimationFrame(() => document.getElementById(`tab-${next.id}`)?.focus());
  };
  return (
    <div
      role="tablist"
      aria-label={label}
      className="flex flex-wrap gap-1 border-b border-slate-800"
      onKeyDown={(e) => {
        if (e.key === "ArrowRight") { e.preventDefault(); move(1); }
        if (e.key === "ArrowLeft") { e.preventDefault(); move(-1); }
      }}
    >
      {tabs.map((t) => {
        const active = t.id === value;
        const Icon = t.icon;
        return (
          <button
            key={t.id}
            id={`tab-${t.id}`}
            type="button"
            role="tab"
            aria-selected={active}
            tabIndex={active ? 0 : -1}
            onClick={() => onChange(t.id)}
            className={`-mb-px inline-flex items-center gap-1.5 border-b-2 px-3 py-2 text-sm font-medium transition-colors ${ui.focus} ${
              active ? "border-emerald-400 text-emerald-300" : "border-transparent text-slate-400 hover:text-slate-200"
            }`}
          >
            {Icon && <Icon className="h-4 w-4" aria-hidden="true" />}
            {t.label}
            {t.count !== undefined && (
              <span className={`rounded-full px-1.5 font-mono text-xs ${active ? "bg-emerald-950 text-emerald-300" : "bg-slate-800 text-slate-400"}`}>
                {t.count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
