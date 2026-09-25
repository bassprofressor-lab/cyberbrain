import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { useT } from "@/lib/i18n";
import { prettyKeys } from "@/lib/keys";

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

export function Section({ id, title, aside, children, className = "" }: { id?: string; title: ReactNode; aside?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section id={id} className={`panel ${className}`}>
      <header className="section-head">
        <h2>{title}</h2>
        {aside ? <div className="text-xs text-fg-muted flex items-center gap-2 min-w-0">{aside}</div> : null}
      </header>
      <div className="p-4">{children}</div>
    </section>
  );
}

export function Stat({ label, value, sub, tone, mono = false }: { label: ReactNode; value: ReactNode; sub?: ReactNode; tone?: "ok" | "warn" | "danger" | undefined; mono?: boolean }) {
  return (
    <div className="stat">
      <div className="label truncate">{label}</div>
      <div className={`stat-value truncate ${mono ? "is-mono" : ""} ${tone === "ok" ? "text-ok" : tone === "warn" ? "text-warn" : tone === "danger" ? "text-danger" : ""}`}>{value}</div>
      {sub ? <div className="mt-1 text-xs text-fg-muted truncate">{sub}</div> : null}
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

export function Empty({ title, children }: { title: ReactNode; children?: ReactNode }) {
  return (
    <div className="empty">
      <div className="empty-title">{title}</div>
      {children ? <div className="empty-hint">{children}</div> : null}
    </div>
  );
}

export function Loading({ label }: { label?: string }) {
  const t = useT();
  return (
    <div className="py-8 text-center text-xs text-fg-muted" role="status">
      {label ?? t.common.loading}…
    </div>
  );
}

export function ErrorBanner({ error, onRetry }: { error: ApiError; onRetry?: () => void }) {
  const t = useT();
  return (
    <div className="alert alert-danger" role="alert">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="font-medium text-danger">
            {error.body.code}
            <span className="ml-2 font-mono text-2xs text-fg-muted">
              {t.error.http(error.status || "—", error.body.exit_code)}
            </span>
          </div>
          <div className="mt-0.5 text-fg">{error.message}</div>
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
