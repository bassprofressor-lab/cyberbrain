/**
 * The design foundation, for every screen to grow into.
 *
 * Tokens live in index.css (`--radius-card`, `--space-*`, the four state colours); this file
 * holds the two components that carry them: `Card` and `StatusBadge`. Only the Status screen
 * and the sidebar use them so far; the other screens still use `Section` and `Pill`, and keep
 * working.
 *
 * Four states, and they are not interchangeable:
 *   ok      green   it works
 *   idle    grey    not set up, and that is a legitimate choice; it says what is missing
 *   warn    orange  something real wants attention (stale index, orphan vectors)
 *   danger  red     it is broken
 * "Not set up" must never be drawn in a warning colour: a screen that is orange all the time
 * teaches people to stop looking at orange.
 */
import type { ReactNode } from "react";
import { Icon, type IconName } from "./icons";

export type State = "ok" | "idle" | "warn" | "danger";

const STATE_ICON: Record<State, IconName> = { ok: "check", idle: "dash", warn: "alert", danger: "cross" };

/** The worst of several states; `idle` ranks below `warn` because it is not a problem. */
export function worst(states: State[]): State {
  for (const s of ["danger", "warn", "idle"] as const) if (states.includes(s)) return s;
  return "ok";
}

/** A word and a shape, never a colour alone. */
export function StatusBadge({ state, children, className = "" }: { state: State; children: ReactNode; className?: string }) {
  return (
    <span className={`sbadge sbadge-${state} ${className}`}>
      <Icon name={STATE_ICON[state]} size={13} strokeWidth={2.2} />
      <span className="truncate">{children}</span>
    </span>
  );
}

/** Round state mark, for the headline. */
export function StateMark({ state, size = 40 }: { state: State; size?: number }) {
  return (
    <span className={`state-mark state-mark-${state}`} style={{ width: size, height: size }} aria-hidden>
      <Icon name={STATE_ICON[state]} size={size * 0.55} strokeWidth={2.2} />
    </span>
  );
}

export function Card({ title, icon, aside, state, children, className = "" }: { title?: ReactNode; icon?: IconName; aside?: ReactNode; state?: State; children: ReactNode; className?: string }) {
  return (
    <section className={`card ${state === "idle" ? "card-idle" : ""} ${className}`}>
      {title ? (
        <header className="card-head">
          {icon ? <Icon name={icon} size={20} className="text-fg-muted" /> : null}
          <h2 className="shrink-0 whitespace-nowrap">{title}</h2>
          {aside ? <div className="card-aside">{aside}</div> : null}
        </header>
      ) : null}
      <div className="card-body">{children}</div>
    </section>
  );
}

/** A figure card: icon tile, label, one large readable value, one line of context. */
export function MetricCard({ icon, label, value, state, sub }: { icon: IconName; label: ReactNode; value: ReactNode; state?: State; sub?: ReactNode }) {
  return (
    <div className={`card metric ${state === "idle" ? "card-idle" : ""}`}>
      <span className={`metric-tile ${state ? `metric-tile-${state}` : ""}`}>
        <Icon name={icon} size={22} />
      </span>
      <div className="min-w-0">
        <div className="metric-label truncate">{label}</div>
        <div className={`metric-value ${state ? `is-${state}` : ""}`}>
          {state ? <Icon name={STATE_ICON[state]} size={18} strokeWidth={2.2} className="metric-glyph" /> : null}
          <span className="min-w-0 wrap-soft">{value}</span>
        </div>
        {sub ? <div className="metric-sub">{sub}</div> : null}
      </div>
    </div>
  );
}

/** The title row of a screen: icon, h1, a quiet subtitle, and the screen's actions on the right. */
export function PageHeader({ icon, title, sub, children, className = "" }: { icon?: IconName; title: ReactNode; sub?: ReactNode; children?: ReactNode; className?: string }) {
  return (
    <div className={`flex items-center gap-x-4 gap-y-3 flex-wrap ${className}`}>
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1 min-w-0 mr-auto">
        {icon ? <Icon name={icon} size={24} className="text-fg-muted self-center" /> : null}
        <h1 className="text-2xl font-semibold tracking-tight">{title}</h1>
        {sub ? <span className="text-sm text-fg-muted min-w-0 wrap-soft basis-full sm:basis-auto">{sub}</span> : null}
      </div>
      {children ? <div className="flex flex-wrap items-center gap-2">{children}</div> : null}
    </div>
  );
}
