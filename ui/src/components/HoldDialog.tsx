import type { PiiHold } from "@/api/client";
import { Pill } from "@/components/ui";
import { relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";

/** A write held for possible personal data (SPEC §12.4). Shared by editing and creating a note. */
export function HoldDialog({ hold, onResolve }: { hold: PiiHold; onResolve: (a: "redact" | "mark-reviewed" | "proceed" | "discard") => void }) {
  const t = useT();
  return (
    <div className="fixed inset-0 z-40 bg-bg/70 flex items-center justify-center p-4" role="dialog" aria-modal aria-label={t.note.hold.aria}>
      <div className="panel shadow-panel w-[min(36rem,100%)]">
        <header className="px-4 h-10 border-b flex items-center gap-2">
          <Pill tone="warn">{t.note.hold.badge}</Pill>
          <h2 className="text-sm font-semibold">{t.note.hold.title(hold.note)}</h2>
        </header>
        <div className="p-4 text-sm space-y-3">
          <p className="text-fg-muted">{t.note.hold.body}</p>
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left label">
                <th className="py-1 font-medium">{t.note.hold.colKind}</th>
                <th className="py-1 font-medium">{t.note.hold.colExcerpt}</th>
                <th className="py-1 font-medium tnum">{t.note.hold.colPos}</th>
              </tr>
            </thead>
            <tbody>
              {hold.findings.map((f, i) => (
                <tr key={i} className="border-t">
                  <td className="py-1"><Pill tone="warn">{f.kind}</Pill></td>
                  <td className="py-1 font-mono">{f.excerpt}</td>
                  <td className="py-1 font-mono tnum text-fg-muted">{f.line}:{f.col}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <div className="flex flex-wrap gap-1.5 justify-end pt-1">
            <button type="button" className="btn btn-sm" onClick={() => onResolve("discard")}>{t.note.hold.discard}</button>
            <button type="button" className="btn btn-sm" onClick={() => onResolve("proceed")} title={t.note.hold.proceedTitle}>{t.note.hold.proceed}</button>
            <button type="button" className="btn btn-sm" onClick={() => onResolve("mark-reviewed")} title={t.note.hold.markReviewedTitle}>{t.note.hold.markReviewed}</button>
            <button type="button" className="btn btn-sm btn-primary" onClick={() => onResolve("redact")}>{t.note.hold.redact}</button>
          </div>
          <div className="text-2xs text-fg-faint">{t.note.hold.expires(relTime(hold.expires_at))}</div>
        </div>
      </div>
    </div>
  );
}
