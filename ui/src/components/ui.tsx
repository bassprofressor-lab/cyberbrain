import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
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
      <header className="flex items-center justify-between gap-3 px-4 h-10 border-b">
        <h2 className="text-sm font-semibold">{title}</h2>
        {aside ? <div className="text-xs text-fg-muted flex items-center gap-2">{aside}</div> : null}
      </header>
      <div className="p-4">{children}</div>
    </section>
  );
}

export function Stat({ label, value, sub, tone, mono = false }: { label: ReactNode; value: ReactNode; sub?: ReactNode; tone?: "ok" | "warn" | "danger" | undefined; mono?: boolean }) {
  return (
    <div className="panel px-3 py-2.5 min-w-0">
      <div className="label truncate">{label}</div>
      <div className={`mt-0.5 text-lg leading-tight tnum truncate ${mono ? "font-mono text-base" : "font-medium"} ${tone === "ok" ? "text-ok" : tone === "warn" ? "text-warn" : tone === "danger" ? "text-danger" : ""}`}>{value}</div>
      {sub ? <div className="mt-0.5 text-xs text-fg-muted truncate">{sub}</div> : null}
    </div>
  );
}

export function Field({ label, children, className = "" }: { label: ReactNode; children: ReactNode; className?: string }) {
  return (
    <div className={`min-w-0 ${className}`}>
      <div className="label">{label}</div>
      <div className="mt-0.5 text-sm break-words">{children}</div>
    </div>
  );
}

export function Pill({ children, tone = "neutral", className = "", title }: { children: ReactNode; tone?: "neutral" | "ok" | "warn" | "danger"; className?: string; title?: string | undefined }) {
  const cls = tone === "ok" ? "text-ok bg-ok-bg border-ok/40" : tone === "warn" ? "text-warn bg-warn-bg border-warn/40" : tone === "danger" ? "text-danger bg-danger-bg border-danger/40" : "text-fg-muted bg-surface-2";
  return (
    <span className={`inline-flex items-center gap-1 h-5 px-1.5 rounded border text-2xs font-medium whitespace-nowrap ${cls} ${className}`} title={title}>
      {children}
    </span>
  );
}

export function Dot({ tone }: { tone: "ok" | "warn" | "danger" | "off" }) {
  const c = tone === "ok" ? "var(--ok)" : tone === "warn" ? "var(--warn)" : tone === "danger" ? "var(--danger)" : "var(--fg-faint)";
  return <span className="inline-block w-2 h-2 rounded-full shrink-0" style={{ background: c, boxShadow: tone === "ok" ? `0 0 0 3px color-mix(in oklch, ${c} 25%, transparent)` : undefined }} aria-hidden />;
}

export function Empty({ title, children }: { title: ReactNode; children?: ReactNode }) {
  return (
    <div className="py-10 text-center">
      <div className="text-sm text-fg-muted">{title}</div>
      {children ? <div className="mt-2 text-xs text-fg-faint">{children}</div> : null}
    </div>
  );
}

export function Loading({ label = "loading" }: { label?: string }) {
  return (
    <div className="py-6 text-center text-xs text-fg-faint" role="status">
      {label}…
    </div>
  );
}

export function ErrorBanner({ error, onRetry }: { error: ApiError; onRetry?: () => void }) {
  return (
    <div className="panel border-danger/50 bg-danger-bg px-4 py-3 text-sm" role="alert">
      <div className="flex items-start gap-3">
        <div className="min-w-0 flex-1">
          <div className="font-medium text-danger">
            {error.body.code}
            <span className="ml-2 font-mono text-2xs text-fg-muted">
              http {error.status || "—"} · exit {error.body.exit_code}
            </span>
          </div>
          <div className="mt-0.5 text-fg">{error.message}</div>
        </div>
        {onRetry ? (
          <button className="btn btn-sm" onClick={onRetry}>
            retry
          </button>
        ) : null}
      </div>
    </div>
  );
}

export function KeyValue({ rows }: { rows: Array<[ReactNode, ReactNode]> }) {
  return (
    <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
      {rows.map(([k, v], i) => (
        <div key={i} className="contents">
          <dt className="text-fg-muted whitespace-nowrap">{k}</dt>
          <dd className="min-w-0 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}
