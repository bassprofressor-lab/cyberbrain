import { useEffect, useRef, useState, type FormEvent } from "react";
import { api, ApiError, type NoteCreateRequest, type NoteDetail, type NoteKind, type PiiHold } from "@/api/client";
import { HoldDialog } from "@/components/HoldDialog";
import { useToast } from "@/components/Toast";
import { ErrorBanner } from "@/components/ui";
import { useT } from "@/lib/i18n";
import { href } from "@/lib/router";
import { isValidName, slugFromTitle } from "@/lib/slug";
import { toApiError } from "@/lib/useAsync";

const KINDS: NoteKind[] = ["knowledge", "bug", "lesson", "decision", "reference", "session"];

/**
 * The rings a note can be created in from the page.
 *
 * Rings 0 and 1 are resident: they are loaded into every agent session on the machine and
 * outrank everything else. Setting one is the operator's decision and stays on the command
 * line, where it is typed on purpose, rather than one select option away from a colleague.
 */
export const CREATABLE_RINGS = [2, 3, 4] as const;
type CreatableRing = (typeof CREATABLE_RINGS)[number];

/**
 * A new note, for both views. The simple view asks for a title and a text and nothing else:
 * ring 2, kind knowledge, no bereich, so nothing it writes is shared or binds an agent. The
 * full view adds ring, kind, bereich and tags.
 *
 * The name is made from the title and shown before saving, because it is the file name and
 * the citation, and it can be changed there.
 */
export function NewNoteForm({ full, onCreated, onCancel }: { full: boolean; onCreated: (d: NoteDetail) => void; onCancel: () => void }) {
  const t = useT();
  const toast = useToast();
  const [title, setTitle] = useState("");
  const [typedName, setTypedName] = useState<string | null>(null);
  const [body, setBody] = useState("");
  const [ring, setRing] = useState<CreatableRing>(2);
  const [kind, setKind] = useState<NoteKind>("knowledge");
  const [bereich, setBereich] = useState("");
  const [tags, setTags] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<ApiError | null>(null);
  const [exists, setExists] = useState<string | null>(null);
  const [hold, setHold] = useState<PiiHold | null>(null);
  const titleRef = useRef<HTMLInputElement>(null);

  useEffect(() => titleRef.current?.focus(), []);

  const name = typedName ?? slugFromTitle(title);
  const nameBad = name !== "" && !isValidName(name);
  const canSave = !saving && name !== "" && !nameBad && body.trim() !== "";

  const created = (d: NoteDetail) => {
    toast(full ? t.note.wrote(d.path) : t.newNote.saved);
    onCreated(d);
  };

  const save = async (e?: FormEvent) => {
    e?.preventDefault();
    if (!canSave) return;
    const front: NoteCreateRequest["front"] = { name, ring: full ? ring : 2, kind: full ? kind : "knowledge" };
    if (full) {
      const tagList = tags.split(",").map((s) => s.trim()).filter(Boolean);
      if (tagList.length) front.tags = tagList;
      if (bereich.trim()) front.bereich = bereich.trim();
    }
    setSaving(true);
    setError(null);
    setExists(null);
    try {
      created(await api.createNote({ body, front }));
    } catch (x) {
      const err = toApiError(x);
      if (err.body.code === "pii-held" && err.body.hold) setHold(err.body.hold);
      // Creation never updates in place: the server refuses an existing name, and the page
      // says so in words instead of showing the code.
      else if (err.body.code === "write-conflict") setExists(name);
      else setError(err);
    } finally {
      setSaving(false);
    }
  };

  const resolveHold = async (action: "redact" | "mark-reviewed" | "proceed" | "discard") => {
    if (!hold) return;
    try {
      const res = await api.resolveHold(hold.hold_id, { action });
      setHold(null);
      if (res) created(res);
      else toast(t.note.hold.discarded, "info");
    } catch (x) {
      setError(toApiError(x));
      setHold(null);
    }
  };

  return (
    <>
      <form aria-label={t.newNote.heading} className="flex flex-col gap-4" onSubmit={save}>
        <div className="flex flex-col gap-1">
          <label htmlFor="new-note-title" className="label">{t.newNote.titleLabel}</label>
          <input id="new-note-title" ref={titleRef} className="input h-9" value={title} placeholder={t.newNote.titlePlaceholder} onChange={(e) => setTitle(e.target.value)} />
        </div>

        <div className="flex flex-col gap-1">
          <label htmlFor="new-note-name" className="label">{t.newNote.nameLabel}</label>
          <input id="new-note-name" className="input h-8 font-mono text-xs" value={name} spellCheck={false} aria-describedby="new-note-name-hint" aria-invalid={nameBad || undefined} onChange={(e) => setTypedName(e.target.value)} />
          <span id="new-note-name-hint" className={`text-2xs ${nameBad ? "text-danger" : "text-fg-faint"}`}>
            {nameBad ? t.newNote.nameInvalid : t.newNote.nameHint}
          </span>
        </div>

        {full ? (
          <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
            <div className="flex flex-col gap-1">
              <label htmlFor="new-note-ring" className="label">{t.note.field.ring}</label>
              <select id="new-note-ring" className="input h-8" value={ring} onChange={(e) => setRing(Number(e.target.value) as CreatableRing)}>
                {CREATABLE_RINGS.map((r) => (
                  <option key={r} value={r}>
                    {`r${r} · ${t.rings.label[r]}`}
                  </option>
                ))}
              </select>
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor="new-note-kind" className="label">{t.notes.kindAria}</label>
              <select id="new-note-kind" className="input h-8" value={kind} onChange={(e) => setKind(e.target.value as NoteKind)}>
                {KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor="new-note-bereich" className="label">{t.note.field.bereich}</label>
              <input id="new-note-bereich" className="input h-8" value={bereich} placeholder={t.note.notShared} title={t.newNote.bereichHint} onChange={(e) => setBereich(e.target.value)} />
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor="new-note-tags" className="label">{t.note.field.tags}</label>
              <input id="new-note-tags" className="input h-8" value={tags} placeholder={t.newNote.tagsHint} onChange={(e) => setTags(e.target.value)} />
            </div>
            <p className="col-span-2 sm:col-span-4 text-2xs text-fg-faint">{t.newNote.ringHint}</p>
          </div>
        ) : null}

        {full ? (
          <textarea className="input w-full h-[40vh] p-3 font-mono text-[13px] leading-relaxed resize-y" value={body} spellCheck={false} aria-label={t.note.bodyAria} onChange={(e) => setBody(e.target.value)} />
        ) : (
          <div className="flex flex-col gap-1">
            <label htmlFor="new-note-text" className="label">{t.newNote.textLabel}</label>
            <textarea id="new-note-text" className="input w-full h-56 p-3 text-sm leading-relaxed resize-y" value={body} placeholder={t.newNote.textPlaceholder} onChange={(e) => setBody(e.target.value)} />
          </div>
        )}

        {exists ? (
          <div className="panel border-danger/50 bg-danger-bg px-4 py-3 text-sm" role="alert">
            {t.newNote.exists(exists)}{" "}
            <a className="link" href={href("note", exists)}>
              {t.newNote.openExisting}
            </a>
          </div>
        ) : null}
        {error ? <ErrorBanner error={error} /> : null}

        <div className="flex gap-1.5 justify-end">
          <button type="button" className="btn btn-sm" onClick={onCancel} disabled={saving}>
            {t.common.cancel}
          </button>
          <button type="submit" className="btn btn-sm btn-primary" disabled={!canSave}>
            {full ? (saving ? t.note.writing : t.note.write) : saving ? t.newNote.saving : t.newNote.save}
          </button>
        </div>
      </form>
      {/* Outside the form: the dialog's buttons would otherwise submit it. */}
      {hold ? <HoldDialog hold={hold} onResolve={resolveHold} /> : null}
    </>
  );
}
