import { api, type NoteSummary } from "@/api/client";
import { RingChip } from "@/components/RingBadge";
import { ErrorBanner, Loading } from "@/components/ui";
import { absTime, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { href, navigate } from "@/lib/router";
import { useAsync } from "@/lib/useAsync";
import { titleOf } from "./SimpleNotes";

/**
 * What was written down most recently, across the store.
 *
 * There is no new call behind this: it is `listNotes(sort: "updated")`, which the notes
 * screen already makes. It exists as its own entry because "what changed while I was away"
 * is a question people actually have and the full view answers only by implication — a list
 * sorted by date is not the same thing as a screen that says so.
 */
export function TeamScreen() {
  const t = useT();
  const list = useAsync<NoteSummary[]>(() => api.listNotes({ sort: "updated" }), []);
  const notes = (list.data ?? []).slice(0, 20);

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-2xl px-8 py-8 flex flex-col gap-5">
        <div className="flex items-baseline gap-3 flex-wrap">
          <h1 className="text-xl font-semibold tracking-tight">{t.team.title}</h1>
          <span className="text-2xs text-fg-faint">{t.team.subtitle}</span>
        </div>

        {list.error ? <ErrorBanner error={list.error} /> : null}
        {list.loading && !list.data ? <Loading /> : null}
        {!list.loading && !notes.length && !list.error ? <p className="text-sm text-fg-muted py-6">{t.team.empty}</p> : null}

        {notes.length ? (
          <div className="flex flex-col gap-1">
            {notes.map((n) => (
              <button key={n.id} className="source-row" onClick={() => navigate(href("note", n.name))}>
                <RingChip ring={n.ring} className="shrink-0 mt-0.5" />
                <span className="min-w-0 flex-1">
                  <span className="block text-sm leading-snug text-pretty">{titleOf(n.name)}</span>
                  <span className="block mt-1 text-2xs text-fg-faint" title={absTime(n.updated)}>
                    {relTime(n.updated)}
                  </span>
                </span>
                <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="text-fg-faint shrink-0 self-center" aria-hidden>
                  <path d="m6.5 4 4 4-4 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                </svg>
              </button>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}
