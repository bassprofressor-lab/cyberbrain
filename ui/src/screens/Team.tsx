import { api, type HubStatus, type NoteSummary } from "@/api/client";
import { RingChip } from "@/components/RingBadge";
import { ErrorBanner, Loading } from "@/components/ui";
import { absTime, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { href, navigate } from "@/lib/router";
import { useAsync } from "@/lib/useAsync";
import { titleOf } from "./SimpleNotes";

/**
 * What this machine shares, and with whom.
 *
 * Until 2026-09-10 this was `listNotes(sort: "updated")` and nothing else, which is why the
 * full view never had it: a list sorted by date is something the notes screen already does.
 * A note now carries a `bereich`, and that changes what this screen is for — it is the one
 * place that answers "what of ours leaves this machine, and what stays", which no other
 * screen asks. That is why it is in both views now.
 *
 * Deliberately not shown here: conflicts and which device sent what. Those live on the hub,
 * and a store cannot answer them without fetching. A screen that quietly reaches across the
 * network to fill a list would make "open a tab" into an egress event.
 */
export function TeamScreen() {
  const t = useT();
  const list = useAsync<NoteSummary[]>(() => api.listNotes({ sort: "updated" }), []);
  const hub = useAsync<HubStatus>(() => api.hubStatus(), []);
  const notes = list.data ?? [];

  // Grouped by bereich, most recently touched group first, so the order follows the work
  // rather than the alphabet.
  const byBereich = new Map<string, NoteSummary[]>();
  const unshared: NoteSummary[] = [];
  for (const n of notes) {
    if (!n.bereich) {
      unshared.push(n);
      continue;
    }
    const g = byBereich.get(n.bereich);
    if (g) g.push(n);
    else byBereich.set(n.bereich, [n]);
  }
  const groups = [...byBereich.entries()];

  const row = (n: NoteSummary) => (
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
  );

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-2xl px-8 py-8 flex flex-col gap-5">
        <div className="flex items-baseline gap-3 flex-wrap">
          <h1 className="text-xl font-semibold tracking-tight">{t.team.title}</h1>
          <span className="text-2xs text-fg-faint">{t.team.subtitle}</span>
        </div>

        {/* Said before the lists, because it decides what they mean: the same note in the
            same bereich is shared on one machine and private on another. */}
        <p className="text-xs text-fg-muted">
          {hub.data?.enrolled ? t.team.enrolled(hub.data.hub ?? "") : t.team.notEnrolled}
        </p>

        {list.error ? <ErrorBanner error={list.error} /> : null}
        {list.loading && !list.data ? <Loading /> : null}
        {!list.loading && !notes.length && !list.error ? <p className="text-sm text-fg-muted py-6">{t.team.empty}</p> : null}

        {groups.map(([bereich, ns]) => (
          <section key={bereich} className="flex flex-col gap-1">
            <h2 className="text-sm font-medium">
              {bereich} <span className="text-2xs text-fg-faint font-normal">{t.team.count(ns.length)}</span>
            </h2>
            {ns.slice(0, 10).map(row)}
          </section>
        ))}

        {unshared.length ? (
          <section className="flex flex-col gap-1">
            <h2 className="text-sm font-medium text-fg-muted">
              {t.team.privateHeading} <span className="text-2xs text-fg-faint font-normal">{t.team.count(unshared.length)}</span>
            </h2>
            {/* The default state, and the safe one. Named rather than left implicit, because
                "nobody else sees this" is the fact somebody came here to check. */}
            <p className="text-2xs text-fg-faint mb-1">{t.team.privateNote}</p>
            {unshared.slice(0, 10).map(row)}
          </section>
        ) : null}
      </div>
    </div>
  );
}
