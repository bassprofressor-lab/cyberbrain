import { useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, type ForgetReport, type NoteDetail, type NoteKind, type NoteSummary, type PiiHold, type Ring } from "@/api/client";
import { CitationChip } from "@/components/Citation";
import { HoldDialog } from "@/components/HoldDialog";
import { NewNoteForm } from "@/components/NewNote";
import { RingBadge, RingGlyph } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Empty, ErrorBanner, Field, Kbd, Loading, Pill } from "@/components/ui";
import { absTime, duration, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { Markdown } from "@/lib/markdown";
import { href, navigate, type Route } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

const KINDS: NoteKind[] = ["knowledge", "bug", "lesson", "decision", "reference", "session"];

export function NoteScreen({ route }: { route: Route }) {
  const t = useT();
  const selectedName = route.screen === "note" ? route.param : null;
  const creating = route.screen === "notes" && route.query.has("new");
  const [filter, setFilter] = useState(route.query.get("q") ?? "");
  const [ring, setRing] = useState<Ring | null>(route.query.has("ring") ? (Number(route.query.get("ring")) as Ring) : null);
  const [kind, setKind] = useState<NoteKind | "">("");
  const list = useAsync(() => {
    const p: Parameters<typeof api.listNotes>[0] = { sort: "updated" };
    if (ring !== null) p.ring = ring;
    if (kind) p.kind = kind;
    if (filter.trim()) p.q = filter.trim();
    return api.listNotes(p);
  }, [ring, kind, filter]);
  const notes = list.data ?? [];
  const names = useMemo(() => new Set(notes.map((n) => n.name)), [notes]);
  const [cursor, setCursor] = useState(0);
  const listRef = useRef<HTMLOListElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    const i = notes.findIndex((n) => n.name === selectedName || n.id === selectedName);
    if (i >= 0) setCursor(i);
  }, [notes, selectedName]);
  useEffect(() => {
    listRef.current?.children[cursor]?.scrollIntoView({ block: "nearest" });
  }, [cursor]);

  useShortcuts(
    "notes",
    editing || creating
      ? []
      : [
          { keys: "j", label: t.notes.keys.next, run: () => setCursor((c) => Math.min(notes.length - 1, c + 1)) },
          { keys: "k", label: t.notes.keys.prev, run: () => setCursor((c) => Math.max(0, c - 1)) },
          { keys: "ArrowDown", label: t.notes.keys.next, run: () => setCursor((c) => Math.min(notes.length - 1, c + 1)), inInputs: true, hidden: true },
          { keys: "ArrowUp", label: t.notes.keys.prev, run: () => setCursor((c) => Math.max(0, c - 1)), inInputs: true, hidden: true },
          { keys: "Enter", label: t.notes.keys.open, run: () => notes[cursor] && navigate(href("note", notes[cursor]?.name)), inInputs: true },
          { keys: "/", label: t.notes.keys.filter, run: () => filterRef.current?.select() },
          { keys: "e", label: t.notes.keys.edit, run: () => selectedName && setEditing(true) },
        ],
    [notes, cursor, selectedName, editing, creating, t],
  );

  return (
    <div className="grid h-full grid-cols-1 md:grid-cols-[19rem_1fr]">
      <aside className="border-r flex flex-col min-h-0 max-h-[40vh] md:max-h-none">
        <div className="p-3 border-b space-y-2">
          <div className="flex gap-1.5">
            <input ref={filterRef} className="input w-full min-w-0" placeholder={t.notes.filterPlaceholder} value={filter} onChange={(e) => setFilter(e.target.value)} aria-label={t.notes.filterAria} />
            <button className="btn btn-sm shrink-0" onClick={() => navigate(href("notes", null, { new: 1 }))}>
              + {t.newNote.button}
            </button>
          </div>
          <div className="flex gap-1 flex-wrap">
            {([null, 0, 1, 2, 3, 4] as Array<Ring | null>).map((r) => (
              <button key={String(r)} className={`btn btn-sm ${ring === r ? "btn-primary" : ""}`} onClick={() => setRing(r)} style={r !== null && ring !== r ? { color: `var(--ring-${r})` } : undefined}>
                {r === null ? t.common.all : `r${r}`}
              </button>
            ))}
            <select className="input h-6 text-xs ml-auto" value={kind} onChange={(e) => setKind(e.target.value as NoteKind | "")} aria-label={t.notes.kindAria}>
              <option value="">{t.notes.anyKind}</option>
              {KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </div>
        </div>
        <ol ref={listRef} className="flex-1 overflow-auto scroll-thin" aria-label={t.notes.listAria}>
          {list.error ? (
            <li className="p-3">
              <ErrorBanner error={list.error} onRetry={list.reload} />
            </li>
          ) : null}
          {notes.map((n, i) => (
            <NoteRow key={n.id} n={n} active={n.name === selectedName || n.id === selectedName} cursor={i === cursor} onHover={() => setCursor(i)} />
          ))}
          {!list.loading && !notes.length ? <Empty title={t.notes.noMatch} /> : null}
        </ol>
        <div className="px-3 py-1.5 border-t text-2xs text-fg-faint tnum">
          {t.notes.count(notes.length)} · <Kbd keys="j" /> <Kbd keys="k" /> <Kbd keys="Enter" />
        </div>
      </aside>
      <main className="min-h-0 overflow-auto scroll-thin">
        {creating ? (
          <div className="p-5 max-w-3xl">
            <h1 className="text-lg font-semibold mb-4">{t.newNote.heading}</h1>
            <NewNoteForm
              full
              onCreated={(d) => {
                list.reload();
                navigate(href("note", d.front.name));
              }}
              onCancel={() => navigate(href("notes"))}
            />
          </div>
        ) : selectedName ? (
          <NoteDetailView key={selectedName} nameOrId={selectedName} resolves={(nm) => names.has(nm)} editing={editing} setEditing={setEditing} block={route.query.get("block")} onChanged={list.reload} />
        ) : (
          <Empty title={t.notes.selectTitle}>
            <Kbd keys="j" /> <Kbd keys="k" /> {t.notes.selectHint.move} <Kbd keys="Enter" /> {t.notes.selectHint.open} <Kbd keys="e" /> {t.notes.selectHint.edit}
          </Empty>
        )}
      </main>
    </div>
  );
}

function NoteRow({ n, active, cursor, onHover }: { n: NoteSummary; active: boolean; cursor: boolean; onHover: () => void }) {
  const t = useT();
  return (
    <li className={`border-b ${active ? "row-selected" : cursor ? "bg-surface-2" : ""}`} onMouseEnter={onHover}>
      <a href={href("note", n.name)} className="block px-3 py-2">
        <div className="flex items-center gap-2">
          <RingGlyph ring={n.ring} size={9} />
          <span className="text-sm font-medium truncate">{n.name}</span>
          {n.dangling ? (
            <span className="ml-auto text-2xs text-fg-faint" title={t.notes.danglingTitle(n.dangling)}>
              {n.dangling}?
            </span>
          ) : null}
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-2xs text-fg-faint tnum">
          <span style={{ color: `var(--ring-${n.ring})` }}>r{n.ring}</span>
          <span>{n.kind}</span>
          <span>{relTime(n.updated)}</span>
          {n.pii !== "none" ? <Pill tone={n.pii === "flagged" ? "danger" : n.pii === "reviewed" ? "warn" : "neutral"}>{n.pii === "unscanned" ? t.notes.piiNotScanned : t.notes.pii(t.pii.state[n.pii])}</Pill> : null}
        </div>
      </a>
    </li>
  );
}

function NoteDetailView({ nameOrId, resolves, editing, setEditing, block, onChanged }: { nameOrId: string; resolves: (n: string) => boolean; editing: boolean; setEditing: (b: boolean) => void; block: string | null; onChanged: () => void }) {
  const t = useT();
  const toast = useToast();
  const note = useAsync(() => api.getNote(nameOrId), [nameOrId]);
  const [draft, setDraft] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<ApiError | null>(null);
  const [hold, setHold] = useState<PiiHold | null>(null);
  const [forget, setForget] = useState<ForgetReport | null>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const d = note.data;

  useEffect(() => {
    if (editing && d) {
      setDraft(d.body);
      setSaveError(null);
      setTimeout(() => taRef.current?.focus(), 0);
    }
  }, [editing, d]);

  // Deep link from a search hit: scroll the cited block's heading into view when possible.
  useEffect(() => {
    if (!d || block === null) return;
    const el = document.getElementById(`block-${block}`);
    el?.scrollIntoView({ block: "start" });
  }, [d, block]);

  const save = async () => {
    if (!d) return;
    setSaving(true);
    setSaveError(null);
    try {
      const updated = await api.writeNote(d.front.name, { body: draft, expected_updated: d.front.updated });
      note.set(updated);
      setEditing(false);
      toast(t.note.wrote(updated.path));
      onChanged();
    } catch (e) {
      const err = toApiError(e);
      if (err.body.code === "pii-held" && err.body.hold) setHold(err.body.hold);
      else setSaveError(err);
    } finally {
      setSaving(false);
    }
  };

  const resolveHold = async (action: "redact" | "mark-reviewed" | "proceed" | "discard") => {
    if (!hold) return;
    try {
      const res = await api.resolveHold(hold.hold_id, { action });
      setHold(null);
      if (res) {
        note.set(res);
        setEditing(false);
        toast(action === "redact" ? t.note.hold.wroteRedacted : action === "mark-reviewed" ? t.note.hold.wroteReviewed : t.note.hold.wroteFlagged);
        onChanged();
      } else toast(t.note.hold.discarded, "info");
    } catch (e) {
      setSaveError(toApiError(e));
      setHold(null);
    }
  };

  const doForget = async (dryRun: boolean) => {
    if (!d) return;
    try {
      const r = await api.forget(d.front.name, dryRun);
      if (dryRun) setForget(r);
      else {
        setForget(null);
        toast(t.note.forgetDialog.erased(r.note.name, { blocks: r.removed.blocks, vectors: r.removed.vectors, fts: r.removed.fts_rows }));
        onChanged();
        navigate(href("notes"));
      }
    } catch (e) {
      setSaveError(toApiError(e));
    }
  };

  useShortcuts(
    "note",
    editing
      ? [
          { keys: "Mod+Enter", label: t.note.keys.save, run: save, inInputs: true },
          { keys: "Escape", label: t.note.keys.cancelEdit, run: () => (hold ? undefined : setEditing(false)), inInputs: true },
        ]
      : [{ keys: "y", label: t.note.keys.copyFirst, run: () => d?.blocks[0] && navigator.clipboard?.writeText(d.blocks[0].citation).then(() => toast(t.citation.copied(d.blocks[0]?.citation ?? ""))) }],
    [editing, draft, d, hold, t],
  );

  if (note.error) return <div className="p-4"><ErrorBanner error={note.error} onRetry={note.reload} /></div>;
  if (!d) return <Loading label={t.note.loading} />;

  const f = d.front;
  const resolvesAll = (nm: string) => resolves(nm) || d.outbound.some((l) => l.target === nm && l.resolved);

  return (
    <div className="grid grid-cols-1 xl:grid-cols-[1fr_17rem] min-h-full">
      <article className="p-5 min-w-0">
        <header className="flex flex-wrap items-center gap-2">
          <RingBadge ring={f.ring} showName />
          <h1 className="text-lg font-semibold font-mono truncate">{f.name}</h1>
          <Pill>{f.kind}</Pill>
          {f.pii !== "none" ? (
            <Pill tone={f.pii === "flagged" ? "danger" : f.pii === "reviewed" ? "warn" : "neutral"} title={f.pii === "unscanned" ? t.note.piiNotScannedTitle : undefined}>
              {f.pii === "unscanned" ? t.note.piiNotScanned : t.notes.pii(t.pii.state[f.pii])}
            </Pill>
          ) : null}
          <div className="ml-auto flex gap-1.5">
            {editing ? (
              <>
                <button className="btn btn-sm" onClick={() => setEditing(false)} disabled={saving}>
                  {t.common.cancel} <Kbd keys="Escape" />
                </button>
                <button className="btn btn-sm btn-primary" onClick={save} disabled={saving}>
                  {saving ? t.note.writing : t.note.write} <Kbd keys="Mod+Enter" className="opacity-70" />
                </button>
              </>
            ) : (
              <>
                <button className="btn btn-sm btn-danger" onClick={() => doForget(true)} title={t.note.forgetTitle}>
                  {t.note.forget}
                </button>
                <button className="btn btn-sm" onClick={() => setEditing(true)}>
                  {t.common.edit} <Kbd keys="e" />
                </button>
              </>
            )}
          </div>
        </header>

        <div className="mt-4 grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-x-4 gap-y-3 panel p-3">
          <Field label={t.note.field.id}>
            <code className="text-xs">{f.id}</code>
          </Field>
          <Field label={t.note.field.path}>
            <code className="text-xs">{d.path}</code>
          </Field>
          <Field label={t.note.field.created}>
            <span title={absTime(f.created)}>{relTime(f.created)}</span>
          </Field>
          <Field label={t.note.field.updated}>
            <span title={absTime(f.updated)}>{relTime(f.updated)}</span>
          </Field>
          <Field label={t.note.field.retention}>{f.retention ? <span title={f.retention}>{duration(f.retention)}</span> : <span className="text-fg-muted">{t.common.indefinite}</span>}</Field>
          {/* Absent is the safe state and says so: a note with no bereich is never
              shared with a hub, whatever else is configured. Shown even when empty,
              because "not shared" is a fact about a note, not the absence of one. */}
          <Field label={t.note.field.bereich}>
            {f.bereich ? <span>{f.bereich}</span> : <span className="text-fg-muted">{t.note.notShared}</span>}
          </Field>
          <Field label={t.note.field.ring}>
            {f.ring} · {t.rings.label[f.ring]}
          </Field>
          <Field label={t.note.field.tags} className="col-span-2">
            {f.tags?.length ? (
              <span className="flex flex-wrap gap-1">
                {f.tags.map((tag) => (
                  <a key={tag} href={href("notes", null, { q: tag })} className="inline-flex h-5 px-1.5 rounded bg-surface-2 text-xs text-fg-muted hover:text-fg">
                    {tag}
                  </a>
                ))}
              </span>
            ) : (
              <span className="text-fg-muted">{t.common.none}</span>
            )}
          </Field>
        </div>

        {saveError ? <div className="mt-3"><ErrorBanner error={saveError} /></div> : null}

        {editing ? (
          <div className="mt-4">
            <textarea
              ref={taRef}
              className="input w-full h-[60vh] p-3 font-mono text-[13px] leading-relaxed resize-y"
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              spellCheck={false}
              aria-label={t.note.bodyAria}
            />
            <div className="mt-1.5 text-2xs text-fg-faint">{t.note.bodyNote}</div>
          </div>
        ) : (
          <div className="mt-5">
            <BlockAnchors detail={d} />
            <Markdown source={d.body} resolves={resolvesAll} />
          </div>
        )}
      </article>

      <aside className="border-t xl:border-t-0 xl:border-l p-4 space-y-5 text-sm">
        <div>
          <div className="label mb-1.5">{t.note.citations(d.blocks.length)}</div>
          <ul className="space-y-1">
            {d.blocks.map((b) => (
              <li key={b.citation} className="flex items-center gap-2 min-w-0">
                <CitationChip citation={b.citation} ring={f.ring} size="sm" />
                <a href={href("note", f.name, { block: b.idx })} className="text-2xs text-fg-faint truncate" title={b.preview}>
                  {b.preview || t.note.blockLabel(b.idx)}
                </a>
              </li>
            ))}
          </ul>
        </div>
        <div>
          <div className="label mb-1.5">{t.note.outbound(d.outbound.length)}</div>
          {d.outbound.length ? (
            <ul className="space-y-1">
              {d.outbound.map((l) => (
                <li key={l.target} className="flex items-center gap-1.5 min-w-0">
                  {l.resolved ? (
                    <>
                      <RingGlyph ring={l.resolved.ring} size={9} />
                      <a href={href("note", l.target)} className="link truncate">
                        {l.target}
                      </a>
                    </>
                  ) : (
                    <>
                      <span className="inline-block w-[9px] h-[9px] rounded-full border border-dashed border-fg-faint shrink-0" aria-hidden />
                      <span className="link-intent truncate" title={t.note.intentTitle}>
                        {l.target}
                      </span>
                      <span className="text-2xs text-fg-faint">{t.common.intent}</span>
                    </>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <div className="text-xs text-fg-faint">{t.common.none}</div>
          )}
        </div>
        <div>
          <div className="label mb-1.5">{t.note.inbound(d.inbound.length)}</div>
          {d.inbound.length ? (
            <ul className="space-y-1">
              {d.inbound.map((l) => (
                <li key={l.from.id} className="flex items-center gap-1.5 min-w-0">
                  <RingGlyph ring={l.from.ring} size={9} />
                  <a href={href("note", l.from.name)} className="link truncate">
                    {l.from.name}
                  </a>
                </li>
              ))}
            </ul>
          ) : (
            <div className="text-xs text-fg-faint">{t.note.noInbound}</div>
          )}
        </div>
      </aside>

      {hold ? <HoldDialog hold={hold} onResolve={resolveHold} /> : null}
      {forget ? <ForgetDialog report={forget} onCancel={() => setForget(null)} onConfirm={() => doForget(false)} /> : null}
    </div>
  );
}

/** Invisible anchors so `?block=N` from a search hit lands near the right paragraph. Approximate: block N ≈ heading N. */
function BlockAnchors({ detail }: { detail: NoteDetail }) {
  return (
    <>
      {detail.blocks.map((b) => (
        <span key={b.idx} id={`block-${b.idx}`} className="block h-0" aria-hidden />
      ))}
    </>
  );
}

function ForgetDialog({ report, onCancel, onConfirm }: { report: ForgetReport; onCancel: () => void; onConfirm: () => void }) {
  const t = useT();
  const r = report.removed;
  return (
    <div className="fixed inset-0 z-40 bg-bg/70 flex items-center justify-center p-4" role="dialog" aria-modal aria-label={t.note.forgetDialog.aria}>
      <div className="panel shadow-panel w-[min(30rem,100%)]">
        <header className="px-4 h-10 border-b flex items-center gap-2">
          <Pill>{t.note.forgetDialog.dryRun}</Pill>
          <h2 className="text-sm font-semibold">{t.note.forgetDialog.title(report.note.name)}</h2>
        </header>
        <div className="p-4 text-sm space-y-3">
          <p className="text-fg-muted">{t.note.forgetDialog.body}</p>
          <ul className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs font-mono tnum">
            <li>{t.note.forgetDialog.file} <span className="text-fg-muted">{report.note.path}</span></li>
            <li>{t.note.forgetDialog.blocks} {r.blocks}</li>
            <li>{t.note.forgetDialog.vectors} {r.vectors}</li>
            <li>{t.note.forgetDialog.ftsRows} {r.fts_rows}</li>
            <li>{t.note.forgetDialog.linksIn} {r.links_in}</li>
            <li>{t.note.forgetDialog.linksOut} {r.links_out}</li>
            <li>{t.note.forgetDialog.derivatives} {r.derivatives}</li>
          </ul>
          {report.notes.length ? (
            <ul className="text-2xs text-fg-muted space-y-0.5">
              {report.notes.map((x, i) => (
                <li key={i}>{x}</li>
              ))}
            </ul>
          ) : null}
          <p className="text-2xs text-fg-faint">{t.note.forgetDialog.audit}</p>
          <div className="flex gap-1.5 justify-end">
            <button className="btn btn-sm" onClick={onCancel}>{t.common.cancel}</button>
            <button className="btn btn-sm btn-danger" onClick={onConfirm}>{t.note.forgetDialog.confirm}</button>
          </div>
        </div>
      </div>
    </div>
  );
}
