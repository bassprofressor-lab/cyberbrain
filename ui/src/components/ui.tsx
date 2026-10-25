import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { useT } from "@/lib/i18n";
import { errorKind } from "@/lib/errors";
import { prettyKeys } from "@/lib/keys";
import { StateMark } from "./design";
import { Icon, type IconName } from "./icons";

export function Kbd({ keys, className = "" }: { keys: string; className?: string }) {
  return (
    <span className={`inline-flex gap-0.5 ${className}`}>
      {prettyKeys(keys).map((k, i) => (
        <kbd key={i} className="kbd">
          {k}
        </kbd>
      ))}
    </span>
  );
}

export function Section({ id, title, icon, aside, children, className = "", fill = false }: { id?: string; title: ReactNode; icon?: IconName; aside?: ReactNode; children: ReactNode; className?: string; fill?: boolean }) {
  // `fill`: the section takes the height it is given and its body is a column, so a child
  // with `flex-1 min-h-0 overflow-y-auto` scrolls inside it. The body used to be a plain
  // block, in which that child just grew, and a long output pushed the input out of view.
  return (
    <section id={id} className={`panel ${fill ? "flex-1 min-h-0 flex flex-col" : ""} ${className}`}>
      <header className="section-head">
        {icon ? <Icon name={icon} size={20} className="text-fg-muted" /> : null}
        <h2 className="shrink-0 whitespace-nowrap">{title}</h2>
        {aside ? <div className="ml-auto text-xs text-fg-muted flex items-center justify-end gap-2 min-w-0 flex-1">{aside}</div> : null}
      </header>
      <div className={`p-5 ${fill ? "flex-1 min-h-0 flex flex-col" : ""}`}>{children}</div>
    </section>
  );
}

export function Stat({ label, value, sub, tone, mono = false, icon }: { label: ReactNode; value: ReactNode; sub?: ReactNode; tone?: "ok" | "warn" | "danger" | "idle" | undefined; mono?: boolean; icon?: IconName }) {
  return (
    <div className={`stat ${icon ? "flex items-start gap-4" : ""}`}>
      {icon ? (
        <span className={`metric-tile ${tone ? `metric-tile-${tone}` : ""}`}>
          <Icon name={icon} size={22} />
        </span>
      ) : null}
      <div className="min-w-0 flex-1">
        <div className="metric-label truncate">{label}</div>
        <div className={`stat-value truncate ${mono ? "is-mono" : ""} ${tone === "ok" ? "text-ok" : tone === "warn" ? "text-warn" : tone === "danger" ? "text-danger" : tone === "idle" ? "text-idle" : ""}`}>{value}</div>
        {sub ? <div className="mt-1 text-sm text-fg-muted wrap-soft">{sub}</div> : null}
      </div>
    </div>
  );
}

export function Field({ label, children, className = "" }: { label: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={`min-w-0 ${className}`}>
      <div className="label">{label}</div>
      <div className="mt-1 text-sm break-words">{children}</div>
    </div>
  );
}

export function Pill({ children, tone = "neutral", className = "", title }: { children: ReactNode; tone?: "neutral" | "ok" | "warn" | "danger"; className?: string; title?: string | undefined }) {
  const cls = tone === "ok" ? "badge-ok" : tone === "warn" ? "badge-warn" : tone === "danger" ? "badge-danger" : "";
  return (
    <span className={`badge ${cls} ${className}`} title={title}>
      {children}
    </span>
  );
}

export function Dot({ tone }: { tone: "ok" | "warn" | "danger" | "off" }) {
  const c = tone === "ok" ? "var(--ok)" : tone === "warn" ? "var(--warn)" : tone === "danger" ? "var(--danger)" : "var(--fg-faint)";
  return <span className="inline-block w-2 h-2 rounded-full shrink-0" style={{ background: c, boxShadow: tone === "ok" ? `0 0 0 3px color-mix(in oklch, ${c} 22%, transparent)` : undefined }} aria-hidden />;
}

export function Empty({ title, children, icon = "info" }: { title: ReactNode; children?: ReactNode; icon?: IconName }) {
  return (
    <div className="empty">
      <span className="metric-tile metric-tile-idle mx-auto mb-3" style={{ width: "2.75rem", height: "2.75rem" }}>
        <Icon name={icon} size={22} />
      </span>
      <div className="empty-title">{title}</div>
      {children ? <div className="empty-hint">{children}</div> : null}
    </div>
  );
}

export function Loading({ label }: { label?: string }) {
  const t = useT();
  return (
    <div className="py-8 px-5 max-w-md mx-auto" role="status">
      <div className="space-y-2.5 animate-pulse" aria-hidden>
        <div className="h-3 rounded-full bg-surface-3 w-2/3" />
        <div className="h-3 rounded-full bg-surface-3 w-full" />
        <div className="h-3 rounded-full bg-surface-3 w-5/6" />
      </div>
      <div className="mt-3 text-sm text-fg-muted text-center">{label ?? t.common.loading}…</div>
    </div>
  );
}

/**
 * What went wrong, in a sentence first; the code, the status and the server's own text are
 * one click down. The server's wording is English and technical, and it used to be the first
 * thing a colleague read.
 */
export function ErrorBanner({ error, onRetry }: { error: ApiError; onRetry?: () => void }) {
  const t = useT();
  return (
    <div className="alert alert-danger" role="alert">
      <div className="flex items-start gap-3">
        <StateMark state="danger" size={28} />
        <div className="min-w-0 flex-1">
          <div className="font-medium text-danger">{t.error.friendly[errorKind(error)]}</div>
          <details className="mt-1 text-xs">
            <summary className="cursor-pointer text-fg-muted">{t.error.details}</summary>
            <div className="mt-1">
              <span className="font-mono">{error.body.code}</span>
              <span className="ml-2 font-mono text-2xs text-fg-muted">{t.error.http(error.status || "—", error.body.exit_code)}</span>
            </div>
            <div className="mt-0.5 text-fg break-words">{error.message}</div>
          </details>
        </div>
        {onRetry ? (
          <button className="btn btn-sm" onClick={onRetry}>
            {t.common.retry}
          </button>
        ) : null}
      </div>
    </div>
  );
}

export function KeyValue({ rows }: { rows: Array<[ReactNode, ReactNode]> }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-5 gap-y-1.5 text-sm">
      {rows.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="text-fg-muted whitespace-nowrap">{k}</dt>
          <dd className="min-w-0 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
