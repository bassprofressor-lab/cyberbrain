import { useState } from "react";
import { api, type HubStatus, type NoteSummary } from "@/api/client";
import { RingChip } from "@/components/RingBadge";
import { Card, PageHeader, StatusBadge } from "@/components/design";
import { Icon } from "@/components/icons";
import { Empty, ErrorBanner, Loading } from "@/components/ui";
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
  // Groups shown in full. Ten is where the list starts; the count in the heading is the true
  // total, and a button gets to the rest rather than the screen quietly stopping at ten.
  const [open, setOpen] = useState<Set<string>>(new Set());
  const PREVIEW = 10;
  const more = (key: string, total: number) =>
    total > PREVIEW ? (
      <button type="button" className="btn btn-ghost btn-sm self-start" aria-expanded={open.has(key)} onClick={() => setOpen((s) => { const x = new Set(s); if (!x.delete(key)) x.add(key); return x; })}>
        {open.has(key) ? t.team.showFewer : t.team.showAll(total)}
      </button>
    ) : null;
  const shownOf = (key: string, ns: NoteSummary[]) => (open.has(key) ? ns : ns.slice(0, PREVIEW));

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
      <Icon name="chevron" size={16} className="-rotate-90 text-fg-faint self-center" />
    </button>
  );

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-3xl px-4 py-5 sm:px-8 sm:py-8 flex flex-col gap-5">
        <PageHeader icon="team" title={t.team.title} sub={t.team.subtitle} />

        {/* Said before the lists, because it decides what they mean: the same note in the
            same bereich is shared on one machine and private on another. */}
        <p className="card px-5 py-3.5 text-sm text-fg-muted flex flex-wrap items-center gap-x-3 gap-y-1">
          {hub.data && !hub.error ? <StatusBadge state={hub.data.enrolled ? "ok" : "idle"}>{hub.data.enrolled ? t.team.badgeEnrolled : t.team.badgeAlone}</StatusBadge> : null}
          {hub.loading && !hub.data ? t.team.hubChecking : hub.error || !hub.data ? t.team.hubUnknown : hub.data.enrolled ? t.team.enrolled(hub.data.hub ?? "") : t.team.notEnrolled}
          {hub.error ? (
            <button type="button" className="btn btn-ghost btn-sm ml-2" onClick={hub.reload}>
              {t.common.retry}
            </button>
          ) : null}
        </p>

        {list.error ? <ErrorBanner error={list.error} /> : null}
        {list.loading && !list.data ? <Loading /> : null}
        {!list.loading && !notes.length && !list.error ? <Empty title={t.team.empty} icon="team" /> : null}

        {groups.map(([bereich, ns]) => (
          <Card key={bereich} title={bereich} icon="folder" aside={ns.length > PREVIEW && !open.has(bereich) ? t.team.showing(PREVIEW, ns.length) : t.team.count(ns.length)}>
            <div className="flex flex-col gap-1 -m-2">
              {shownOf(bereich, ns).map(row)}
              {more(bereich, ns.length)}
            </div>
          </Card>
        ))}

        {unshared.length ? (
          <Card title={t.team.privateHeading} icon="compliance" state="idle" aside={unshared.length > PREVIEW && !open.has("") ? t.team.showing(PREVIEW, unshared.length) : t.team.count(unshared.length)}>
            <div className="flex flex-col gap-1">
            {/* The default state, and the safe one. Named rather than left implicit, because
                "nobody else sees this" is the fact somebody came here to check. */}
            <p className="text-sm text-fg-muted mb-1">{t.team.privateNote}</p>
            <div className="flex flex-col gap-1 -mx-2">
              {shownOf("", unshared).map(row)}
              {more("", unshared.length)}
            </div>
            </div>
          </Card>
        ) : null}
      </div>
    </div>
  );
}
