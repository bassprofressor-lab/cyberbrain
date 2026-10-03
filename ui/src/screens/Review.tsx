import { useState } from "react";
import { api, ApiError, type ProposalDecision, type ProposalDetail } from "@/api/client";
import { RingBadge } from "@/components/RingBadge";
import { ErrorBanner, Loading, Pill } from "@/components/ui";
import { absTime, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { Markdown } from "@/lib/markdown";
import { useAsync } from "@/lib/useAsync";

/**
 * What agents want to add to memory, and who decided (C5, 2026-10-03).
 *
 * The one screen where a person is the second key. Everything a decision needs is on the
 * card, and in the order it is needed: whose proposal it is and for whom, whether the text
 * came from outside, whether the file is still what was proposed, what AgentGuard said, and
 * then the text itself. Accepting and rejecting are the same `review` the CLI runs; the
 * reviewer is this machine's identity, so the server refuses the proposer deciding their own.
 */
export function ReviewScreen() {
  const t = useT();
  const waiting = useAsync<ProposalDetail[]>(() => api.proposals(), []);
  const history = useAsync<ProposalDecision[]>(() => api.proposalHistory(30), []);
  const reload = () => {
    waiting.reload();
    history.reload();
  };
  const items = waiting.data ?? [];

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-3xl px-8 py-8 flex flex-col gap-6">
        <div className="flex items-baseline gap-3 flex-wrap">
          <h1 className="text-xl font-semibold tracking-tight">{t.review.title}</h1>
          <span className="text-2xs text-fg-faint">{t.review.subtitle}</span>
          <button className="btn btn-sm ml-auto" onClick={reload}>
            {t.common.refresh}
          </button>
        </div>

        <section className="flex flex-col gap-3">
          <h2 className="text-sm font-medium">
            {t.review.waiting} <span className="text-2xs text-fg-faint font-normal">{items.length}</span>
          </h2>
          {waiting.error ? <ErrorBanner error={waiting.error} onRetry={waiting.reload} /> : null}
          {waiting.loading && !waiting.data ? <Loading /> : null}
          {!waiting.loading && !items.length && !waiting.error ? <p className="text-sm text-fg-muted py-2">{t.review.none}</p> : null}
          {items.map((p) => (
            <ProposalCard key={p.name} p={p} onDecided={reload} />
          ))}
        </section>

        <section className="flex flex-col gap-2">
          <h2 className="text-sm font-medium">{t.review.history}</h2>
          {history.error ? <ErrorBanner error={history.error} onRetry={history.reload} /> : null}
          {!history.loading && !(history.data ?? []).length && !history.error ? <p className="text-sm text-fg-muted">{t.review.noHistory}</p> : null}
          {(history.data ?? []).length ? (
            <table className="w-full text-xs">
              <thead className="text-fg-faint text-left">
                <tr>
                  <th className="font-normal py-1 pr-3">{t.review.colWhen}</th>
                  <th className="font-normal py-1 pr-3">{t.review.colNote}</th>
                  <th className="font-normal py-1 pr-3">{t.review.colProposedBy}</th>
                  <th className="font-normal py-1 pr-3">{t.review.colDecidedBy}</th>
                  <th className="font-normal py-1">{t.review.colOutcome}</th>
                </tr>
              </thead>
              <tbody>
                {(history.data ?? []).map((d, i) => (
                  <tr key={`${d.name}-${d.ts}-${i}`} className="border-t border-line align-top">
                    <td className="py-1.5 pr-3 whitespace-nowrap text-fg-muted" title={absTime(d.ts)}>
                      {relTime(d.ts)}
                    </td>
                    <td className="py-1.5 pr-3 font-mono break-all">{d.name}</td>
                    <td className="py-1.5 pr-3 font-mono break-all">{d.proposed_by ?? "—"}</td>
                    <td className="py-1.5 pr-3 font-mono break-all">{d.by ?? "—"}</td>
                    <td className="py-1.5">
                      <Pill tone={d.accepted ? "ok" : "warn"}>{d.accepted ? t.review.accepted : t.review.rejected}</Pill>
                      {d.reason ? <span className="block mt-1 text-fg-muted">{d.reason}</span> : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null}
        </section>
      </div>
    </div>
  );
}

function ProposalCard({ p, onDecided }: { p: ProposalDetail; onDecided: () => void }) {
  const t = useT();
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);

  const decide = async (accept: boolean) => {
    if (!accept && !reason.trim()) {
      setError(new ApiError(0, { code: "bad-request", message: t.review.reasonNeeded, exit_code: 1 }));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.decideProposal(p.name, accept, reason.trim());
      onDecided();
    } catch (e) {
      setError(e instanceof ApiError ? e : new ApiError(0, { code: "internal", message: String(e), exit_code: 2 }));
    } finally {
      setBusy(false);
    }
  };

  const ag = p.agentguard;
  const agText = ag?.error
    ? t.review.agError(ag.error)
    : ag?.action_id
      ? t.review.agState(ag.approval_status ?? ag.outcome ?? "?")
      : null;

  return (
    <article className="rounded-sm border border-line bg-surface p-4 flex flex-col gap-3">
      <header className="flex items-start gap-2 flex-wrap">
        <RingBadge ring={p.ring} />
        <span className="font-mono text-sm break-all">{p.name}</span>
        <span className="text-2xs text-fg-faint ml-auto" title={absTime(p.created)}>
          {relTime(p.created)}
        </span>
      </header>

      <div className="flex flex-wrap gap-1.5 text-2xs">
        <Pill title={p.on_behalf_of ? t.review.onBehalfOf(p.on_behalf_of) : undefined}>{t.review.by(p.proposed_by ?? t.review.unknownProposer)}</Pill>
        {p.untrusted ? <Pill tone="warn">{t.review.untrusted(p.untrusted)}</Pill> : null}
        {p.bereich ? <Pill>{t.review.bereich(p.bereich)}</Pill> : null}
        {p.changes_existing ? <Pill tone="warn">{t.review.changesExisting}</Pill> : null}
        {p.intact === false ? <Pill tone="danger">{t.review.tampered}</Pill> : null}
        {agText ? <Pill tone={ag?.error ? "danger" : "neutral"}>{agText}</Pill> : null}
      </div>

      {p.untrusted ? <p className="text-xs text-fg-muted">{t.review.untrustedNote}</p> : null}

      <div className="text-sm border-l-2 border-line pl-3">
        {/* `[[…]]` in a proposal is shown as intent, never as a link into the store: the text may be from outside. */}
        <Markdown source={p.body} resolves={() => false} />
      </div>

      {error ? <ErrorBanner error={error} /> : null}

      <div className="flex flex-wrap gap-2 items-center">
        <button className="btn btn-sm btn-primary" disabled={busy || p.intact === false || !p.proposed_by} onClick={() => decide(true)}>
          {t.review.accept}
        </button>
        <input
          className="input flex-1 min-w-40"
          placeholder={t.review.reasonPlaceholder}
          aria-label={t.review.reasonPlaceholder}
          value={reason}
          onChange={(e) => setReason(e.target.value)}
        />
        <button className="btn btn-sm btn-danger" disabled={busy} onClick={() => decide(false)}>
          {t.review.reject}
        </button>
      </div>
    </article>
  );
}
