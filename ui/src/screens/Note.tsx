import { useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, type ForgetReport, type NoteDetail, type NoteKind, type NoteSummary, type PiiHold, type Ring } from "@/api/client";
import { CitationChip } from "@/components/Citation";
import { RingBadge, RingGlyph } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Empty, ErrorBanner, Field, Kbd, Loading, Pill } from "@/components/ui";
import { absTime, duration, relTime, RING_LABEL } from "@/lib/format";
import { useShortcuts } from "@/lib/keys";
import { Markdown } from "@/lib/markdown";
import { href, navigate, type Route } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

const KINDS: NoteKind[] = ["knowledge", "bug", "lesson", "decision", "reference", "session"];

export function NoteScreen({ route }: { route: Route }) {
  const selectedName = route.screen === "note" ? route.param : null;
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
    editing
      ? []
      : [
          { keys: "j", label: "Next note", run: () => setCursor((c) => Math.min(notes.length - 1, c + 1)) },
          { keys: "k", label: "Previous note", run: () => setCursor((c) => Math.max(0, c - 1)) },
          { keys: "ArrowDown", label: "Next note", run: () => setCursor((c) => Math.min(notes.length - 1, c + 1)), inInputs: true, hidden: true },
          { keys: "ArrowUp", label: "Previous note", run: () => setCursor((c) => Math.max(0, c - 1)), inInputs: true, hidden: true },
          { keys: "Enter", label: "Open note under cursor", run: () => notes[cursor] && navigate(href("note", notes[cursor]?.name)), inInputs: true },
          { keys: "/", label: "Filter notes", run: () => filterRef.current?.select() },
          { keys: "e", label: "Edit open note", run: () => selectedName && setEditing(true) },
        ],
    [notes, cursor, selectedName, editing],
  );

  return (
    <div className="grid h-full grid-cols-1 md:grid-cols-[19rem_1fr]">
      <aside className="border-r flex flex-col min-h-0 max-h-[40vh] md:max-h-none">
        <div className="p-3 border-b space-y-2">
          <input ref={filterRef} className="input w-full" placeholder="filter by name or tag" value={filter} onChange={(e) => setFilter(e.target.value)} aria-label="Filter notes" />
          <div className="flex gap-1 flex-wrap">
            {([null, 0, 1, 2, 3, 4] as Array<Ring | null>).map((r) => (
              <button key={String(r)} className={`btn btn-sm ${ring === r ? "btn-primary" : ""}`} onClick={() => setRing(r)} style={r !== null && ring !== r ? { color: `var(--ring-${r})` } : undefined}>
                {r === null ? "all" : `r${r}`}
              </button>
            ))}
            <select className="input h-6 text-xs ml-auto" value={kind} onChange={(e) => setKind(e.target.value as NoteKind | "")} aria-label="Kind">
              <option value="">any kind</option>
              {KINDS.map((k) => (
                <option key={k} value={k}>
                  {k}
                </option>
              ))}
            </select>
          </div>
        </div>
        <ol ref={listRef} className="flex-1 overflow-auto scroll-thin" aria-label="Notes">
          {list.error ? (
            <li className="p-3">
              <ErrorBanner error={list.error} onRetry={list.reload} />
            </li>
          ) : null}
          {notes.map((n, i) => (
            <NoteRow key={n.id} n={n} active={n.name === selectedName || n.id === selectedName} cursor={i === cursor} onHover={() => setCursor(i)} />
          ))}
          {!list.loading && !notes.length ? <Empty title="no notes match" /> : null}
        </ol>
        <div className="px-3 py-1.5 border-t text-2xs text-fg-faint tnum">
          {notes.length} note{notes.length === 1 ? "" : "s"} · <Kbd keys="j" /> <Kbd keys="k" /> <Kbd keys="Enter" />
        </div>
      </aside>
      <main className="min-h-0 overflow-auto scroll-thin">
        {selectedName ? (
          <NoteDetailView key={selectedName} nameOrId={selectedName} resolves={(nm) => names.has(nm)} editing={editing} setEditing={setEditing} block={route.query.get("block")} onChanged={list.reload} />
        ) : (
          <Empty title="Select a note.">
            <Kbd keys="j" /> <Kbd keys="k" /> to move, <Kbd keys="Enter" /> to open, <Kbd keys="e" /> to edit.
          </Empty>
        )}
      </main>
    </div>
  );
}

function NoteRow({ n, active, cursor, onHover }: { n: NoteSummary; active: boolean; cursor: boolean; onHover: () => void }) {
  return (
    <li className={`border-b ${active ? "row-selected" : cursor ? "bg-surface-2" : ""}`} onMouseEnter={onHover}>
      <a href={href("note", n.name)} className="block px-3 py-2">
        <div className="flex items-center gap-2">
          <RingGlyph ring={n.ring} size={9} />
          <span className="text-sm font-medium truncate">{n.name}</span>
          {n.dangling ? (
            <span className="ml-auto text-2xs text-fg-faint" title={`${n.dangling} dangling link${n.dangling === 1 ? "" : "s"} (intent)`}>
              {n.dangling}?
            </span>
          ) : null}
        </div>
        <div className="mt-0.5 flex items-center gap-2 text-2xs text-fg-faint tnum">
          <span style={{ color: `var(--ring-${n.ring})` }}>r{n.ring}</span>
          <span>{n.kind}</span>
          <span>{relTime(n.updated)}</span>
          {n.pii !== "none" ? <Pill tone={n.pii === "flagged" ? "danger" : n.pii === "reviewed" ? "warn" : "neutral"}>{n.pii === "unscanned" ? "not scanned" : `pii ${n.pii}`}</Pill> : null}
        </div>
      </a>
    </li>
  );
}

function NoteDetailView({ nameOrId, resolves, editing, setEditing, block, onChanged }: { nameOrId: string; resolves: (n: string) => boolean; editing: boolean; setEditing: (b: boolean) => void; block: string | null; onChanged: () => void }) {
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
      toast(`wrote ${updated.path}`);
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
        toast(action === "redact" ? "written with redactions" : action === "mark-reviewed" ? "written, findings marked reviewed" : "written, note flagged");
        onChanged();
      } else toast("edit discarded, nothing written", "info");
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
        toast(`erased ${r.note.name}: ${r.removed.blocks} blocks, ${r.removed.vectors} vectors, ${r.removed.fts_rows} fts rows`);
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
          { keys: "Mod+Enter", label: "Save", run: save, inInputs: true },
          { keys: "Escape", label: "Cancel edit", run: () => (hold ? undefined : setEditing(false)), inInputs: true },
        ]
      : [{ keys: "y", label: "Copy first citation", run: () => d?.blocks[0] && navigator.clipboard?.writeText(d.blocks[0].citation).then(() => toast(`copied ${d.blocks[0]?.citation}`)) }],
    [editing, draft, d, hold],
  );

  if (note.error) return <div className="p-4"><ErrorBanner error={note.error} onRetry={note.reload} /></div>;
  if (!d) return <Loading label="loading note" />;

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
            <Pill tone={f.pii === "flagged" ? "danger" : f.pii === "reviewed" ? "warn" : "neutral"} title={f.pii === "unscanned" ? "No write-time scan ever ran over this note. Writing it through the tool scans it." : undefined}>
              {f.pii === "unscanned" ? "pii not scanned" : `pii ${f.pii}`}
            </Pill>
          ) : null}
          <div className="ml-auto flex gap-1.5">
            {editing ? (
              <>
                <button className="btn btn-sm" onClick={() => setEditing(false)} disabled={saving}>
                  cancel <Kbd keys="Escape" />
                </button>
                <button className="btn btn-sm btn-primary" onClick={save} disabled={saving}>
                  {saving ? "writing…" : "write to disk"} <Kbd keys="Mod+Enter" className="opacity-70" />
                </button>
              </>
            ) : (
              <>
                <button className="btn btn-sm btn-danger" onClick={() => doForget(true)} title="Runs forget with --dry-run first and shows what would go">
                  forget…
                </button>
                <button className="btn btn-sm" onClick={() => setEditing(true)}>
                  edit <Kbd keys="e" />
                </button>
              </>
            )}
          </div>
        </header>

        <div className="mt-4 grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-x-4 gap-y-3 panel p-3">
          <Field label="id">
            <code className="text-xs">{f.id}</code>
          </Field>
          <Field label="path">
            <code className="text-xs">{d.path}</code>
          </Field>
          <Field label="created">
            <span title={absTime(f.created)}>{relTime(f.created)}</span>
          </Field>
          <Field label="updated">
            <span title={absTime(f.updated)}>{relTime(f.updated)}</span>
          </Field>
          <Field label="retention">{f.retention ? <span title={f.retention}>{duration(f.retention)}</span> : <span className="text-fg-muted">indefinite</span>}</Field>
          <Field label="ring">
            {f.ring} · {RING_LABEL[f.ring]}
          </Field>
          <Field label="tags" className="col-span-2">
            {f.tags?.length ? (
              <span className="flex flex-wrap gap-1">
                {f.tags.map((t) => (
                  <a key={t} href={href("notes", null, { q: t })} className="inline-flex h-5 px-1.5 rounded bg-surface-2 text-xs text-fg-muted hover:text-fg">
                    {t}
                  </a>
                ))}
              </span>
            ) : (
              <span className="text-fg-muted">none</span>
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
              aria-label="Note body"
            />
            <div className="mt-1.5 text-2xs text-fg-faint">
              Body only. Frontmatter fields are edited above the fold in a later version; <code>id</code>, <code>created</code>, <code>updated</code>, <code>links</code> and <code>pii</code> are server-owned. The write goes through the PII scan for profile eu/ch and is reindexed in the same request.
            </div>
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
          <div className="label mb-1.5">citations · {d.blocks.length} blocks</div>
          <ul className="space-y-1">
            {d.blocks.map((b) => (
              <li key={b.citation} className="flex items-center gap-2 min-w-0">
                <CitationChip citation={b.citation} ring={f.ring} size="sm" />
                <a href={href("note", f.name, { block: b.idx })} className="text-2xs text-fg-faint truncate" title={b.preview}>
                  {b.preview || `block ${b.idx}`}
                </a>
              </li>
            ))}
          </ul>
        </div>
        <div>
          <div className="label mb-1.5">outbound · {d.outbound.length}</div>
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
                      <span className="link-intent truncate" title="No note with this name yet. Intent, not an error.">
                        {l.target}
                      </span>
                      <span className="text-2xs text-fg-faint">intent</span>
                    </>
                  )}
                </li>
              ))}
            </ul>
          ) : (
            <div className="text-xs text-fg-faint">none</div>
          )}
        </div>
        <div>
          <div className="label mb-1.5">inbound · {d.inbound.length}</div>
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
            <div className="text-xs text-fg-faint">nothing links here</div>
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

function HoldDialog({ hold, onResolve }: { hold: PiiHold; onResolve: (a: "redact" | "mark-reviewed" | "proceed" | "discard") => void }) {
  return (
    <div className="fixed inset-0 z-40 bg-bg/70 flex items-center justify-center p-4" role="dialog" aria-modal aria-label="Write held: personal data found">
      <div className="panel shadow-panel w-[min(36rem,100%)]">
        <header className="px-4 h-10 border-b flex items-center gap-2">
          <Pill tone="warn">write held</Pill>
          <h2 className="text-sm font-semibold">Possible personal data in {hold.note}</h2>
        </header>
        <div className="p-4 text-sm space-y-3">
          <p className="text-fg-muted">
            Profile <code>eu</code> holds the write until you decide (SPEC §12.4). Detection is heuristic: a seatbelt, not a guarantee. Nothing has been written yet.
          </p>
          <table className="w-full text-xs">
            <thead>
              <tr className="text-left label">
                <th className="py-1 font-medium">kind</th>
                <th className="py-1 font-medium">excerpt (masked)</th>
                <th className="py-1 font-medium tnum">line:col</th>
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
            <button className="btn btn-sm" onClick={() => onResolve("discard")}>discard edit</button>
            <button className="btn btn-sm" onClick={() => onResolve("proceed")} title="Write as-is; note is marked pii: flagged">proceed, flag note</button>
            <button className="btn btn-sm" onClick={() => onResolve("mark-reviewed")} title="Write as-is; note is marked pii: reviewed">write, mark reviewed</button>
            <button className="btn btn-sm btn-primary" onClick={() => onResolve("redact")}>redact and write</button>
          </div>
          <div className="text-2xs text-fg-faint">Hold expires {relTime(hold.expires_at)}; after that the write must be resubmitted.</div>
        </div>
      </div>
    </div>
  );
}

function ForgetDialog({ report, onCancel, onConfirm }: { report: ForgetReport; onCancel: () => void; onConfirm: () => void }) {
  const r = report.removed;
  return (
    <div className="fixed inset-0 z-40 bg-bg/70 flex items-center justify-center p-4" role="dialog" aria-modal aria-label="Forget note">
      <div className="panel shadow-panel w-[min(30rem,100%)]">
        <header className="px-4 h-10 border-b flex items-center gap-2">
          <Pill>dry run</Pill>
          <h2 className="text-sm font-semibold">forget {report.note.name}</h2>
        </header>
        <div className="p-4 text-sm space-y-3">
          <p className="text-fg-muted">The real forget path ran with a no-op writer. This is what it would remove, in one transaction:</p>
          <ul className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs font-mono tnum">
            <li>file <span className="text-fg-muted">{report.note.path}</span></li>
            <li>blocks {r.blocks}</li>
            <li>vectors {r.vectors}</li>
            <li>fts rows {r.fts_rows}</li>
            <li>inbound links {r.links_in}</li>
            <li>outbound links {r.links_out}</li>
            <li>derivatives {r.derivatives}</li>
          </ul>
          {report.notes.length ? (
            <ul className="text-2xs text-fg-muted space-y-0.5">
              {report.notes.map((x, i) => (
                <li key={i}>{x}</li>
              ))}
            </ul>
          ) : null}
          <p className="text-2xs text-fg-faint">Audit rows <code>note.erase.requested</code> and <code>note.erase.completed</code> are written. Notes that link here keep their <code>[[link]]</code>; it becomes intent.</p>
          <div className="flex gap-1.5 justify-end">
            <button className="btn btn-sm" onClick={onCancel}>cancel</button>
            <button className="btn btn-sm btn-danger" onClick={onConfirm}>erase for real</button>
          </div>
        </div>
      </div>
    </div>
  );
}
