import type { PiiHold } from "@/api/client";
import { Dialog } from "@/components/Dialog";
import { Pill } from "@/components/ui";
import { relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";

/** A write held for possible personal data (SPEC §12.4). Shared by editing and creating a note. */
export function HoldDialog({ hold, onResolve }: { hold: PiiHold; onResolve: (a: "redact" | "mark-reviewed" | "proceed" | "discard") => void }) {
  const t = useT();
  return (
    <Dialog
      label={t.note.hold.aria}
      // Escape is the one answer that writes nothing: the held text is thrown away and the
      // editor still has what was typed.
      onEscape={() => onResolve("discard")}
      title={
        <>
          <Pill tone="warn">{t.note.hold.badge}</Pill>
          <h2 className="text-sm font-semibold ml-2">{t.note.hold.title(hold.note)}</h2>
        </>
      }
      actions={
        <>
          <button type="button" className="btn btn-sm" onClick={() => onResolve("discard")}>{t.note.hold.discard}</button>
          <button type="button" className="btn btn-sm" onClick={() => onResolve("proceed")} title={t.note.hold.proceedTitle}>{t.note.hold.proceed}</button>
          <button type="button" className="btn btn-sm" onClick={() => onResolve("mark-reviewed")} title={t.note.hold.markReviewedTitle}>{t.note.hold.markReviewed}</button>
          <button type="button" className="btn btn-sm btn-primary" onClick={() => onResolve("redact")}>{t.note.hold.redact}</button>
        </>
      }
    >
      <p className="text-fg-muted">{t.note.hold.body}</p>
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>{t.note.hold.colKind}</th>
              <th>{t.note.hold.colExcerpt}</th>
              <th className="num">{t.note.hold.colPos}</th>
            </tr>
          </thead>
          <tbody>
            {hold.findings.map((f, i) => (
              <tr key={i}>
                <td><Pill tone="warn">{t.enums.pii[f.kind]}</Pill></td>
                <td className="font-mono">{f.excerpt}</td>
                <td className="num font-mono text-fg-muted">{f.line}:{f.col}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="text-2xs text-fg-faint">{t.note.hold.expires(relTime(hold.expires_at))}</div>
    </Dialog>
  );
}
