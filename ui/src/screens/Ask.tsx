import { useEffect, useRef, useState } from "react";
import { api, ApiError, type RecallResult, type StatusReport } from "@/api/client";
import { RingChip } from "@/components/RingBadge";
import { ErrorBanner } from "@/components/ui";
import { relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { setMode } from "@/lib/mode";
import { href, navigate, type Route } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

/**
 * Simple mode's search: a question, one answer, and where it came from.
 *
 * The expert screen shows a list of hits ranked by a fused score and leaves the reader to
 * decide which of them counts. That decision is the one thing the rings already answer, and
 * it is the wrong decision to hand to somebody who has never read the spec — so here the
 * first hit *is* the answer, at reading size, and everything else is demoted to a source
 * under it.
 *
 * Nothing on this screen shows a citation, a ring number or a score. That is not a
 * simplification of the data: it is the same recall call, and the full form is one switch
 * away.
 */
export function AskScreen({ route }: { route: Route }) {
  const t = useT();
  const inputRef = useRef<HTMLInputElement>(null);
  const [q, setQ] = useState(route.query.get("q") ?? "");
  const [result, setResult] = useState<RecallResult | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(false);
  const gen = useRef(0);

  // The address is the state of record here too, so a question can be sent to a colleague.
  useEffect(() => {
    const timer = setTimeout(() => navigate(href("search", null, { q: q || undefined }), true), 150);
    return () => clearTimeout(timer);
  }, [q]);

  useEffect(() => {
    const query = q.trim();
    if (!query) {
      setResult(null);
      setError(null);
      setLoading(false);
      return;
    }
    const g = ++gen.current;
    setLoading(true);
    const timer = setTimeout(() => {
      api.recall({ q: query, n: 6 }).then(
        (r) => {
          if (g !== gen.current) return;
          setResult(r);
          setError(null);
          setLoading(false);
        },
        (e) => {
          if (g !== gen.current) return;
          setError(toApiError(e));
          setLoading(false);
        },
      );
    }, 220);
    return () => clearTimeout(timer);
  }, [q]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useShortcuts("search", [{ keys: "/", label: t.search.keys.focus, run: () => inputRef.current?.select() }, { keys: "Mod+k", label: t.search.keys.focus, run: () => inputRef.current?.select(), inInputs: true, hidden: true }], [t]);

  const hits = result?.hits ?? [];
  const answer = hits[0];
  const sources = hits.slice(1, 4);
  const open = (name: string, block: number) => navigate(href("note", name, { block }));

  return (
    <div className="h-full overflow-auto scroll-thin">
      <div className="max-w-2xl px-8 py-8 flex flex-col gap-6">
        <div>
          <div className="ask-field">
            <svg width="18" height="18" viewBox="0 0 16 16" fill="none" className="text-fg-faint shrink-0" aria-hidden>
              <circle cx="7" cy="7" r="4.6" stroke="currentColor" strokeWidth="1.5" />
              <path d="M10.4 10.4 14 14" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
            <input ref={inputRef} className="flex-1 bg-transparent border-0 outline-none text-lg placeholder:text-fg-faint" placeholder={t.ask.placeholder} value={q} onChange={(e) => setQ(e.target.value)} spellCheck={false} autoComplete="off" aria-label={t.ask.aria} />
            {loading ? <span className="text-2xs text-fg-faint shrink-0">{t.ask.thinking}</span> : null}
          </div>
          <p className="mt-2 text-2xs text-fg-faint">{t.ask.hint}</p>
        </div>

        {!q.trim() ? (
          <div className="flex flex-wrap gap-2">
            {t.ask.examples.map((ex) => (
              <button key={ex} className="chip-quiet" onClick={() => setQ(ex)}>
                {ex}
              </button>
            ))}
          </div>
        ) : null}

        {error ? <ErrorBanner error={error} /> : null}

        {answer ? (
          <>
            <article className="answer" style={{ ["--wash" as string]: `var(--ring-${answer.ring}-bg)` }}>
              <RingChip ring={answer.ring} />
              <p className="text-xl leading-snug font-medium tracking-tight text-pretty">{answer.text}</p>
              <div className="flex items-center gap-3 flex-wrap pt-3 border-t text-2xs text-fg-muted">
                <span>{t.ask.applies[answer.ring]}</span>
                <button className="btn btn-sm ml-auto" onClick={() => open(answer.note_name, answer.block_idx)}>
                  {t.ask.openNote}
                </button>
              </div>
            </article>

            {result?.conflicts.length ? (
              <div className="rounded-lg border border-warn/40 bg-warn-bg px-4 py-3 text-sm" role="note">
                <span className="font-medium">{t.ask.conflict}</span>{" "}
                <span className="text-fg-muted">{t.ask.conflictWhy}</span>
              </div>
            ) : null}

            {sources.length ? (
              <section className="flex flex-col gap-1">
                <h2 className="label">{t.ask.sources}</h2>
                {sources.map((h) => (
                  <button key={h.citation} className="source-row" onClick={() => open(h.note_name, h.block_idx)}>
                    <RingChip ring={h.ring} className="shrink-0 mt-0.5" />
                    <span className="min-w-0 flex-1">
                      <span className="block text-sm leading-snug line-clamp-3">{h.text}</span>
                      <span className="block mt-1 text-2xs text-fg-faint">{h.note_name.replace(/-/g, " ")}</span>
                    </span>
                    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className="text-fg-faint shrink-0 self-center" aria-hidden>
                      <path d="m6.5 4 4 4-4 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
                    </svg>
                  </button>
                ))}
              </section>
            ) : null}
          </>
        ) : null}

        {q.trim() && !loading && !error && result && !hits.length ? (
          <div className="py-6">
            <p className="text-sm">{t.ask.nothing(q.trim())}</p>
            <p className="mt-1 text-2xs text-fg-faint">{t.ask.nothingHint}</p>
          </div>
        ) : null}

        <HealthLine />
      </div>
    </div>
  );
}

/**
 * The whole of the Status screen that a colleague can act on: whether anything needs
 * attention. Everything behind that answer is a click away, in the mode where it means
 * something.
 */
function HealthLine() {
  const t = useT();
  const s = useAsync<StatusReport>(() => api.status(), []);
  const d = s.data;
  const healthy = d ? d.index.stale_notes === 0 && d.index.orphan_vectors === 0 && d.embedding.matches_index !== false && d.index.fts_ok : true;

  return (
    <div className="mt-2 pt-4 border-t flex items-center gap-2.5 text-2xs text-fg-muted">
      <span className="inline-block w-2 h-2 rounded-full shrink-0" style={{ background: s.loading ? "var(--fg-faint)" : healthy ? "var(--ok)" : "var(--warn)" }} aria-hidden />
      <span>{s.loading ? t.ask.checking : healthy ? t.ask.allGood : t.ask.trouble}</span>
      {d?.index.last_scan ? <span className="text-fg-faint">{t.ask.changed(relTime(d.index.last_scan))}</span> : null}
      <button
        className="btn btn-sm ml-auto"
        onClick={() => {
          setMode("expert");
          navigate(href("status"));
        }}
      >
        {t.ask.details}
      </button>
    </div>
  );
}
