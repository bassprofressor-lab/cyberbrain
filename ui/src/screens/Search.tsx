import { useEffect, useMemo, useRef, useState } from "react";
import { api, ApiError, type Hit, type RecallResult, type Ring } from "@/api/client";
import { CitationChip } from "@/components/Citation";
import { RingBadge, RingLegend } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Empty, ErrorBanner, Kbd, Pill } from "@/components/ui";
import { copyText } from "@/lib/clipboard";
import { highlight } from "@/lib/format";
import { useShortcuts } from "@/lib/keys";
import { href, navigate, type Route } from "@/lib/router";
import { toApiError } from "@/lib/useAsync";

const EXAMPLES = ["postgres data directory", "hook budget", "why no hf-hub", "windows cmd.exe quoting", "contradiction lower ring"];

export function SearchScreen({ route }: { route: Route }) {
  const toast = useToast();
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLOListElement>(null);
  const [q, setQ] = useState(route.query.get("q") ?? "");
  const [n, setN] = useState(Number(route.query.get("n") ?? 8) || 8);
  const [ring, setRing] = useState<Ring | null>(route.query.has("ring") ? (Number(route.query.get("ring")) as Ring) : null);
  const [result, setResult] = useState<RecallResult | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(false);
  const [sel, setSel] = useState(0);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const gen = useRef(0);

  // The URL is the state of record so a search can be bookmarked and shared with an agent.
  useEffect(() => {
    const t = setTimeout(() => navigate(href("search", null, { q: q || undefined, n: n !== 8 ? n : undefined, ring: ring ?? undefined }), true), 150);
    return () => clearTimeout(t);
  }, [q, n, ring]);

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
    const t = setTimeout(() => {
      const params: Parameters<typeof api.recall>[0] = { q: query, n };
      if (ring !== null) params.ring = ring;
      api.recall(params).then(
        (r) => {
          if (g !== gen.current) return;
          setResult(r);
          setError(null);
          setLoading(false);
          setSel(0);
          setExpanded(new Set());
        },
        (e) => {
          if (g !== gen.current) return;
          setError(toApiError(e));
          setLoading(false);
        },
      );
    }, 220);
    return () => clearTimeout(t);
  }, [q, n, ring]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  useEffect(() => {
    listRef.current?.children[sel]?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const hits = result?.hits ?? [];
  const top = hits[0]?.score ?? 1;
  const current = hits[sel];

  const copyCitation = async (h: Hit | undefined) => {
    if (!h) return;
    (await copyText(h.citation)) ? toast(`copied ${h.citation}`) : toast("clipboard unavailable", "err");
  };
  const copyAll = async () => {
    if (!hits.length) return;
    const text = hits.map((h) => h.citation).join("\n");
    (await copyText(text)) ? toast(`copied ${hits.length} citations`) : toast("clipboard unavailable", "err");
  };
  const move = (d: number) => {
    if (!hits.length) return;
    setSel((s) => Math.min(hits.length - 1, Math.max(0, s + d)));
    if (document.activeElement === inputRef.current) inputRef.current?.blur();
  };
  const openSelected = () => {
    if (current) navigate(href("note", current.note_name, { block: current.block_idx }));
  };
  const toggleExpand = (c: string) =>
    setExpanded((s) => {
      const x = new Set(s);
      x.has(c) ? x.delete(c) : x.add(c);
      return x;
    });

  useShortcuts(
    "search",
    [
      { keys: "/", label: "Focus query", run: () => inputRef.current?.select() },
      { keys: "Mod+k", label: "Focus query", run: () => inputRef.current?.select(), inInputs: true, hidden: true },
      { keys: "ArrowDown", label: "Next hit", run: () => move(1), inInputs: true },
      { keys: "ArrowUp", label: "Previous hit", run: () => move(-1), inInputs: true },
      { keys: "j", label: "Next hit", run: () => move(1) },
      { keys: "k", label: "Previous hit", run: () => move(-1) },
      { keys: "c", label: "Copy citation of selected hit", run: () => copyCitation(current) },
      { keys: "y", label: "Copy citation of selected hit", run: () => copyCitation(current), hidden: true },
      { keys: "Shift+C", label: "Copy all citations", run: copyAll },
      { keys: "Enter", label: "Open note of selected hit", run: openSelected },
      { keys: "e", label: "Expand / collapse selected hit", run: () => current && toggleExpand(current.citation) },
      { keys: "Escape", label: "Back to query", run: () => inputRef.current?.select(), inInputs: true },
    ],
    [hits, sel, current],
  );

  const ringOptions = useMemo(() => [null, 0, 1, 2, 3, 4] as Array<Ring | null>, []);

  return (
    <div className="flex flex-col h-full">
      <div className="px-5 pt-4 pb-3 border-b bg-bg sticky top-0 z-10">
        <div className="flex gap-2 items-center">
          <div className="relative flex-1">
            <input
              ref={inputRef}
              className="input w-full h-9 pl-3 pr-16 text-sm"
              placeholder="Recall… (hybrid: lexical + semantic, fused, ring-weighted)"
              value={q}
              onChange={(e) => setQ(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && hits.length) {
                  e.preventDefault();
                  inputRef.current?.blur();
                  setSel(0);
                }
              }}
              spellCheck={false}
              autoComplete="off"
              aria-label="Recall query"
            />
            <span className="absolute right-2 top-1/2 -translate-y-1/2 text-fg-faint text-2xs hidden sm:inline-flex gap-1">
              <Kbd keys="/" />
            </span>
          </div>
          <select className="input h-9" value={n} onChange={(e) => setN(Number(e.target.value))} aria-label="Number of hits" title="n — hits returned">
            {[8, 16, 32, 64].map((v) => (
              <option key={v} value={v}>
                n = {v}
              </option>
            ))}
          </select>
        </div>
        <div className="mt-2 flex items-center gap-1 flex-wrap">
          <span className="label mr-1">ring</span>
          {ringOptions.map((r) => (
            <button
              key={String(r)}
              className={`btn btn-sm ${ring === r ? "btn-primary" : ""}`}
              onClick={() => setRing(r)}
              aria-pressed={ring === r}
              style={r !== null && ring !== r ? { color: `var(--ring-${r})` } : undefined}
            >
              {r === null ? "all" : `r${r}`}
            </button>
          ))}
          <span className="ml-auto text-2xs text-fg-faint tnum">
            {loading ? "searching…" : result ? `${result.mode} · ${result.elapsed_ms} ms · k_lex ${result.params.k_lex} · k_sem ${result.params.k_sem}` : ""}
          </span>
        </div>
      </div>

      <div className="flex-1 overflow-auto scroll-thin px-5 py-4">
        {error ? <ErrorBanner error={error} /> : null}

        {!q.trim() ? (
          <div className="max-w-2xl mx-auto mt-10">
            <Empty title="Type to recall. Every hit carries a citation you can paste back to an agent.">
              <div className="mt-4 flex flex-wrap justify-center gap-1.5">
                {EXAMPLES.map((ex) => (
                  <button key={ex} className="btn btn-sm" onClick={() => setQ(ex)}>
                    {ex}
                  </button>
                ))}
              </div>
            </Empty>
            <div className="panel p-4 mt-6 grid gap-4 sm:grid-cols-2 text-xs">
              <div>
                <div className="label mb-2">keys</div>
                <ul className="space-y-1.5">
                  {[
                    ["/", "focus query"],
                    ["↓ ↑ or j k", "move selection"],
                    ["c", "copy citation of selected hit"],
                    ["Shift+C", "copy all citations, one per line"],
                    ["↵", "open the note at that block"],
                    ["e", "expand selected block"],
                    ["?", "all shortcuts"],
                  ].map(([k, v]) => (
                    <li key={k} className="flex justify-between gap-3">
                      <span className="text-fg-muted">{v}</span>
                      <span className="flex gap-0.5">
                        {(k as string).split(" ").map((x, i) => (
                          <kbd key={i} className="kbd">
                            {x}
                          </kbd>
                        ))}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
              <div>
                <div className="label mb-2">rings</div>
                <RingLegend className="flex-col !gap-y-1.5" />
                <p className="mt-3 text-fg-faint leading-relaxed">
                  Score = RRF over lexical and semantic ranks × ring weight [2.0 1.6 1.0 0.8 0.5]. Lower ring wins a contradiction; both citations are shown.
                </p>
              </div>
            </div>
          </div>
        ) : null}

        {result && q.trim() ? (
          <>
            {result.conflicts.length ? (
              <div className="panel border-warn/50 mb-3 px-4 py-3" role="note">
                <div className="flex items-center gap-2 text-sm font-medium text-warn">
                  {result.conflicts.length} contradiction{result.conflicts.length === 1 ? "" : "s"} reported — lower ring wins, both kept
                </div>
                <ul className="mt-2 space-y-2">
                  {result.conflicts.map((c, i) => (
                    <li key={i} className="text-xs">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-fg-muted w-12">wins</span>
                        <CitationChip citation={c.winner} size="sm" />
                        <span className="text-fg-muted w-12 sm:ml-3">loses</span>
                        <CitationChip citation={c.loser} size="sm" />
                      </div>
                      <div className="mt-1 text-fg-muted">{c.reason}</div>
                    </li>
                  ))}
                </ul>
              </div>
            ) : null}

            {hits.length === 0 && !loading ? (
              <Empty title={`No block matched "${q}"${ring !== null ? ` in ring ${ring}` : ""}.`}>
                {ring !== null ? "Try all rings." : "Recall is over blocks, not titles. Try the words that would appear in the paragraph."}
              </Empty>
            ) : null}

            <ol ref={listRef} className="space-y-1.5" aria-label="Hits">
              {hits.map((h, i) => {
                const isSel = i === sel;
                const open = expanded.has(h.citation);
                return (
                  <li
                    key={h.citation}
                    className={`panel px-3 py-2 cursor-default ${isSel ? "row-selected" : ""}`}
                    onClick={() => setSel(i)}
                    onDoubleClick={() => navigate(href("note", h.note_name, { block: h.block_idx }))}
                    aria-selected={isSel}
                  >
                    <div className="flex items-center gap-2 flex-wrap">
                      <span className="font-mono text-2xs text-fg-faint tnum w-5">{i + 1}</span>
                      <RingBadge ring={h.ring} />
                      <a href={href("note", h.note_name, { block: h.block_idx })} className="link font-medium text-sm truncate max-w-[40%]" onClick={(e) => e.stopPropagation()}>
                        {h.note_name}
                      </a>
                      <span className="text-2xs text-fg-faint tnum">#{h.block_idx}</span>
                      <CitationChip citation={h.citation} ring={h.ring} className="ml-auto" />
                      <ScoreBar score={h.score} top={top} />
                      <span className="hidden md:inline-flex gap-1">
                        {h.sources.map((s) => (
                          <Pill key={s}>{s === "lexical" ? "lex" : "sem"}</Pill>
                        ))}
                      </span>
                    </div>
                    <div className={`mt-1.5 pl-7 text-sm leading-relaxed text-fg whitespace-pre-wrap break-words ${open ? "" : "line-clamp-4"}`} onClick={() => toggleExpand(h.citation)}>
                      {highlight(h.text, q).map((p, j) => (p.hit ? <mark key={j}>{p.t}</mark> : <span key={j}>{p.t}</span>))}
                    </div>
                  </li>
                );
              })}
            </ol>

            {result.caveats.length ? (
              <ul className="mt-3 space-y-0.5">
                {result.caveats.map((c, i) => (
                  <li key={i} className={`text-xs ${/skipped|disabled|not configured/i.test(c) ? "text-warn" : "text-fg-faint"}`}>
                    {c}
                  </li>
                ))}
              </ul>
            ) : null}
          </>
        ) : null}
      </div>
    </div>
  );
}

function ScoreBar({ score, top }: { score: number; top: number }) {
  const pct = top > 0 ? Math.max(4, Math.round((score / top) * 100)) : 0;
  return (
    <span className="inline-flex items-center gap-1.5 w-24" title={`fused, ring-weighted score ${score}`}>
      <span className="h-1.5 flex-1 rounded-sm bg-surface-3 overflow-hidden">
        <span className="block h-full bg-fg-muted" style={{ width: `${pct}%` }} />
      </span>
      <span className="font-mono text-2xs text-fg-muted tnum w-10 text-right">{score.toFixed(4)}</span>
    </span>
  );
}
