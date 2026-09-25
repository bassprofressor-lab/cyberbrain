import type { PiiHold } from "@/api/client";
import { Pill } from "@/components/ui";
import { relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";

/** A write held for possible personal data (SPEC §12.4). Shared by editing and creating a note. */
export function HoldDialog({ hold, onResolve }: { hold: PiiHold; onResolve: (a: "redact" | "mark-reviewed" | "proceed" | "discard") => void }) {
  const t = useT();
  return (
    <div className="scrim items-center justify-center" role="dialog" aria-modal aria-label={t.note.hold.aria}>
      <div className="dialog w-[min(36rem,100%)]">
        <header className="section-head justify-start">
          <Pill tone="warn">{t.note.hold.badge}</Pill>
          <h2 className="text-sm font-semibold">{t.note.hold.title(hold.note)}</h2>
        </header>
        <div className="p-4 text-sm space-y-3">
          <p className="text-fg-muted">{t.note.hold.body}</p>
          <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>{t.note.hold.colKind}</th>
                <th>{t.note.hold.colExcerpt}</th>
                <th className="tnum">{t.note.hold.colPos}</th>
              </tr>
            </thead>
            <tbody>
              {hold.findings.map((f, i) => (
                <tr key={i}>
                  <td><Pill tone="warn">{f.kind}</Pill></td>
                  <td className="font-mono">{f.excerpt}</td>
                  <td className="font-mono tnum text-fg-muted">{f.line}:{f.col}</td>
                </tr>
              ))}
            </tbody>
          </table>
          </div>
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
