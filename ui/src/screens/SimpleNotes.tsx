import { useState } from "react";
import { api, type NoteDetail, type NoteSummary } from "@/api/client";
import { NewNoteForm } from "@/components/NewNote";
import { RingChip } from "@/components/RingBadge";
import { PageHeader } from "@/components/design";
import { Icon } from "@/components/icons";
import { Empty, ErrorBanner, Loading } from "@/components/ui";
import { absTime, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { Markdown } from "@/lib/markdown";
import { cameFromSearch, href, navigate, type Route } from "@/lib/router";
import { useBlockJump } from "@/lib/useBlockJump";
import { splitTitle, titleOf } from "@/lib/slug";
import { useAsync } from "@/lib/useAsync";

export { titleOf };

/**
 * Simple mode's notes: what is written down, as cards, newest first.
 *
 * The expert screen is a two-pane list with ring, kind and tag filters and an editor. This
 * one answers a different question — "what do we have written down" — and answers it without
 * asking the reader to pick a ring first.
 */
export function SimpleNotesScreen({ route }: { route: Route }) {
  const t = useT();
  const [filter, setFilter] = useState("");
  const list = useAsync<NoteSummary[]>(() => api.listNotes({ sort: "updated" }), []);

  if (route.screen === "note" && route.param) return <SimpleNoteView name={route.param} block={route.query.get("block")} />;
  if (route.screen === "notes" && route.query.has("new")) return <SimpleNewNote onCreated={list.reload} />;

  const all = list.data ?? [];
  const notes = filter.trim() ? all.filter((n) => titleOf(n.name).toLowerCase().includes(filter.trim().toLowerCase())) : all;

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-4xl px-4 py-5 sm:px-8 sm:py-8 flex flex-col gap-5">
        <PageHeader icon="notes" title={t.simpleNotes.title} sub={all.length ? <span className="tnum">{t.simpleNotes.count(all.length)}</span> : null}>
          {all.length ? <input className="input w-48" placeholder={t.simpleNotes.filterPlaceholder} aria-label={t.simpleNotes.filterAria} value={filter} onChange={(e) => setFilter(e.target.value)} /> : null}
          <button className="btn btn-primary" onClick={() => navigate(href("notes", null, { new: 1 }))}>
            {t.newNote.button}
          </button>
        </PageHeader>

        {list.error ? <ErrorBanner error={list.error} /> : null}
        {list.loading && !list.data ? <Loading /> : null}

        {!list.loading && !all.length ? <FirstDay /> : null}
        {all.length && !notes.length ? <Empty title={t.simpleNotes.noMatch} icon="search" /> : null}

        {notes.length ? (
          <div className="grid gap-3 grid-cols-[repeat(auto-fill,minmax(15rem,1fr))]">
            {notes.map((n) => (
              <button key={n.id} className="note-card" onClick={() => navigate(href("note", n.name))}>
                <RingChip ring={n.ring} />
                <span className="text-sm font-medium leading-snug text-pretty">{titleOf(n.name)}</span>
                <span className="mt-auto text-2xs text-fg-faint tnum" title={absTime(n.updated)}>
                  {relTime(n.updated)}
                </span>
              </button>
            ))}
          </div>
        ) : null}
      </div>
    </div>
  );
}

/**
 * The empty store, which is what a colleague sees on their first day.
 *
 * The expert screen says "no note matches", which is true of a filter and reads as one: it
 * suggests the notes are there and hidden. This says the store is empty, offers to write the
 * first note, and says how notes usually arrive: mostly the assistant writes them.
 */
function FirstDay() {
  const t = useT();
  const [how, setHow] = useState(false);
  return (
    <div className="panel px-8 py-10 text-center">
      <span className="metric-tile metric-tile-idle mx-auto mb-4"><Icon name="notes" size={24} /></span>
      <h2 className="text-lg font-semibold tracking-tight">{t.simpleNotes.emptyTitle}</h2>
      <p className="mt-2 mx-auto max-w-[44ch] text-sm text-fg-muted">{t.simpleNotes.emptyBody}</p>
      <div className="mt-5 flex justify-center gap-2 flex-wrap">
        <button className="btn btn-primary" onClick={() => navigate(href("notes", null, { new: 1 }))}>
          {t.newNote.firstNote}
        </button>
        <button className="btn" onClick={() => setHow((v) => !v)} aria-expanded={how}>
          {t.simpleNotes.emptyHow}
        </button>
      </div>
      {how ? (
        <div className="mt-4 mx-auto max-w-[52ch] text-left">
          <p className="text-sm text-fg-muted">{t.simpleNotes.emptyHowBody}</p>
          <pre className="mt-2 panel-inset px-3 py-2 text-2xs overflow-x-auto">cyberbrain write --ring 2 --kind knowledge --name my-first-note --body "…"</pre>
        </div>
      ) : null}
    </div>
  );
}

/** Writing a note in the simple view: a title and a text, into ring 2, shared with nobody. */
function SimpleNewNote({ onCreated }: { onCreated: () => void }) {
  const t = useT();
  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-3xl px-4 py-5 sm:px-8 sm:py-8 flex flex-col gap-5">
        <button className="btn btn-ghost btn-sm self-start -ml-2.5" onClick={() => navigate(href("notes"))}>
          <Icon name="chevron" size={16} className="rotate-90" /> {t.simpleNotes.title}
        </button>
        <PageHeader icon="notes" title={t.newNote.heading} />
        <NewNoteForm
          full={false}
          onCreated={(d) => {
            onCreated();
            navigate(href("note", d.front.name));
          }}
          onCancel={() => navigate(href("notes"))}
        />
      </div>
    </div>
  );
}

/**
 * One note, read-only and full width. Editing, forgetting and the frontmatter stay in the
 * full view: this is the screen somebody lands on from a source under an answer, and what
 * they need there is to read it.
 */
function SimpleNoteView({ name, block }: { name: string; block: string | null }) {
  const t = useT();
  const note = useAsync<NoteDetail>(() => api.getNote(name), [name]);
  const d = note.data;
  useBlockJump(d, block);
  // Back goes to the question when that is where the person came from, not to the list of
  // all notes, which is a different place to the one they were reading.
  const fromQuestion = cameFromSearch();

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-3xl px-4 py-5 sm:px-8 sm:py-8 flex flex-col gap-5">
        <button className="btn btn-ghost btn-sm self-start -ml-2.5" onClick={() => (fromQuestion ? history.back() : navigate(href("notes")))}>
          <Icon name="chevron" size={16} className="rotate-90" /> {fromQuestion ? t.simpleNotes.backToQuestion : t.simpleNotes.title}
        </button>
        {note.error ? <ErrorBanner error={note.error} /> : null}
        {note.loading && !d ? <Loading /> : null}
        {d ? (
          <>
            <div className="flex items-center gap-3 flex-wrap">
              <RingChip ring={d.front.ring} />
              <span className="text-2xs text-fg-faint">{t.ask.applies[d.front.ring]}</span>
            </div>
            <h1 className="text-2xl font-semibold tracking-tight text-pretty">{splitTitle(d.front.name, d.body).title}</h1>
            <Markdown source={splitTitle(d.front.name, d.body).text} lineOffset={splitTitle(d.front.name, d.body).skipped} resolves={(n) => d.outbound.some((l) => l.target === n && l.resolved !== null)} className="prose-note" />
            <p className="pt-4 border-t text-2xs text-fg-faint" title={absTime(d.front.updated)}>
              {t.ask.changed(relTime(d.front.updated))}
            </p>
          </>
        ) : null}
      </div>
    </div>
  );
}
