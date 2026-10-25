import { useState } from "react";
import { api, ApiError, type ProposalDecision, type ProposalDetail } from "@/api/client";
import { RingBadge } from "@/components/RingBadge";
import { PageHeader, StatusBadge } from "@/components/design";
import { Icon } from "@/components/icons";
import { Empty, ErrorBanner, Loading, Pill } from "@/components/ui";
import { absTime, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { Markdown } from "@/lib/markdown";
import { diffSummary, lineDiff } from "@/lib/diff";
import { toApiError, useAsync } from "@/lib/useAsync";

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
      <div className="max-w-4xl px-4 py-5 sm:px-8 sm:py-8 flex flex-col gap-6">
        <PageHeader icon="review" title={t.review.title} sub={t.review.subtitle}>
          <button className="btn" onClick={reload}>
            <Icon name="refresh" size={18} />
            {t.common.refresh}
          </button>
        </PageHeader>

        <section className="flex flex-col gap-3">
          <h2 className="flex items-center gap-2 text-base font-semibold">
            {t.review.waiting} <StatusBadge state={items.length ? "warn" : "ok"}>{items.length}</StatusBadge>
          </h2>
          {waiting.error ? <ErrorBanner error={waiting.error} onRetry={waiting.reload} /> : null}
          {waiting.loading && !waiting.data ? <Loading /> : null}
          {!waiting.loading && !items.length && !waiting.error ? <Empty title={t.review.none} icon="check" /> : null}
          {items.map((p) => (
            <ProposalCard key={p.name} p={p} onDecided={reload} />
          ))}
        </section>

        <section className="flex flex-col gap-3">
          <h2 className="text-base font-semibold">{t.review.history}</h2>
          {history.error ? <ErrorBanner error={history.error} onRetry={history.reload} /> : null}
          {!history.loading && !(history.data ?? []).length && !history.error ? <Empty title={t.review.noHistory} icon="review" /> : null}
          {(history.data ?? []).length ? (
            <div className="table-wrap scroll-thin"><table className="table min-w-[40rem]">
              <thead>
                <tr>
                  <th>{t.review.colWhen}</th>
                  <th>{t.review.colNote}</th>
                  <th>{t.review.colProposedBy}</th>
                  <th>{t.review.colDecidedBy}</th>
                  <th>{t.review.colOutcome}</th>
                </tr>
              </thead>
              <tbody>
                {(history.data ?? []).map((d, i) => (
                  <tr key={`${d.name}-${d.ts}-${i}`}>
                    <td className="whitespace-nowrap text-fg-muted" title={absTime(d.ts)}>
                      {relTime(d.ts)}
                    </td>
                    <td className="font-mono break-all">{d.name}</td>
                    <td className="font-mono text-xs break-all">{d.proposed_by ?? "—"}</td>
                    <td className="font-mono text-xs break-all">{d.by ?? "—"}</td>
                    <td>
                      <StatusBadge state={d.accepted ? "ok" : "idle"}>{d.accepted ? t.review.accepted : t.review.rejected}</StatusBadge>
                      {d.reason ? <span className="block mt-1 text-fg-muted">{d.reason}</span> : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table></div>
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
    <article className="card p-5 flex flex-col gap-4">
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

      {/* The reviewer used to see only the new text of a proposal that replaces a note, and
          had to open the note in another tab to find out what was being replaced. */}
      {p.changes_existing ? <Compare name={p.name} proposed={p.body} /> : null}

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

/**
 * What an accepted proposal would change: the note as it is now against the proposal, line
 * by line. Loaded when asked for, not with every card.
 */
function Compare({ name, proposed }: { name: string; proposed: string }) {
  const t = useT();
  const [open, setOpen] = useState(false);
  const [current, setCurrent] = useState<string | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [busy, setBusy] = useState(false);

  const toggle = async () => {
    const next = !open;
    setOpen(next);
    if (!next || current !== null) return;
    setBusy(true);
    setError(null);
    try {
      setCurrent((await api.getNote(name)).body);
    } catch (e) {
      setError(toApiError(e));
    } finally {
      setBusy(false);
    }
  };

  const diff = current === null ? null : lineDiff(current, proposed);
  const sum = diff ? diffSummary(diff) : null;

  return (
    <div className="flex flex-col gap-2">
      <button type="button" className="btn btn-sm self-start" aria-expanded={open} onClick={toggle}>
        {open ? t.review.compareHide : t.review.compare}
      </button>
      {open && busy ? <Loading /> : null}
      {open && error ? <ErrorBanner error={error} onRetry={toggle} /> : null}
      {open && current !== null ? (
        <div className="panel-inset text-xs">
          {sum ? <div className="px-3 py-1.5 border-b text-fg-muted">{t.review.compareSummary(sum.added, sum.removed)}</div> : null}
          {diff ? (
            <pre className="px-3 py-2 overflow-auto max-h-[24rem] whitespace-pre-wrap break-words font-mono" tabIndex={0} aria-label={t.review.compareAria}>
              {diff.map((l, i) => (
                <div key={i} className={l.kind === "add" ? "diff-add" : l.kind === "del" ? "diff-del" : "text-fg-muted"}>
                  <span aria-hidden className="select-none inline-block w-4">{l.kind === "add" ? "+" : l.kind === "del" ? "−" : " "}</span>
                  <span className="sr-only">{l.kind === "add" ? t.review.added : l.kind === "del" ? t.review.removed : t.review.unchanged}: </span>
                  {l.text}
                </div>
              ))}
            </pre>
          ) : (
            <div className="grid gap-3 sm:grid-cols-2 p-3">
              <div>
                <div className="label mb-1">{t.review.before}</div>
                <pre className="whitespace-pre-wrap break-words max-h-[24rem] overflow-auto" tabIndex={0}>{current}</pre>
              </div>
              <div>
                <div className="label mb-1">{t.review.after}</div>
                <pre className="whitespace-pre-wrap break-words max-h-[24rem] overflow-auto" tabIndex={0}>{proposed}</pre>
              </div>
            </div>
          )}
        </div>
      ) : null}
    </div>
  );
}
