import { useMemo, useState } from "react";
import { api, type DayBucket, type LoadedModel, type LoadSummary, type TaskUsage, type UsageTotals } from "@/api/client";
import { ErrorBanner, Loading, Pill, Section, Stat } from "@/components/ui";
import { bytes, num, relTime } from "@/lib/format";
import { useShortcuts } from "@/lib/keys";
import { href, navigate, type Route } from "@/lib/router";
import { useAsync } from "@/lib/useAsync";

/**
 * Two series per chart, and they are the project's own ring hues rather than a new palette:
 * teal `--ring-2` and amber `--ring-3`. Measured with the data-viz validator against both
 * surfaces — light passes every check; dark passes chroma, CVD separation (ΔE 11.3 protan,
 * 21.8 unsimulated) and contrast, and sits above the reference lightness band on purpose,
 * because this surface (L 0.205) is darker than the one that band was cut for.
 *
 * Colour is never the only carrier: every chart has a legend, the stacked segments are held
 * apart by a 2px gap in the surface colour, and the hover panel names both numbers.
 */
const A = "var(--ring-2)";
const B = "var(--ring-3)";

const RANGES = [7, 30, 90];

type Series = { label: string; key: (d: DayBucket) => number; color: string };

/** `2026-09-05` → `05.09.`; the year is in the range label, not on every tick. */
function tick(date: string): string {
  const [, m, d] = date.split("-");
  return `${d}.${m}.`;
}

/**
 * Stacked daily bars. Stacked because both pairs here are part-to-whole — what was read of
 * what there was, what came from cache of what was sent — and a part shown beside its whole
 * invites the reader to add them up a second time.
 */
function StackedDays({ days, lower, upper, unit, empty }: { days: DayBucket[]; lower: Series; upper: Series; unit: string; empty: string }) {
  const [hover, setHover] = useState<number | null>(null);
  const W = 720;
  const H = 190;
  const PAD = { l: 46, r: 8, t: 10, b: 22 };
  const plotW = W - PAD.l - PAD.r;
  const plotH = H - PAD.t - PAD.b;
  const max = Math.max(1, ...days.map((d) => lower.key(d) + upper.key(d)));
  const slot = plotW / Math.max(1, days.length);
  const barW = Math.max(2, Math.min(18, slot - 3));
  const y = (v: number) => PAD.t + plotH - (v / max) * plotH;
  const gridAt = [0, 0.5, 1].map((f) => f * max);
  const anyData = days.some((d) => lower.key(d) + upper.key(d) > 0);
  const h = hover !== null ? days[hover] : undefined;

  return (
    <div className="relative">
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full h-auto" role="img" aria-label={`${lower.label} and ${upper.label} per day`}>
        {gridAt.map((v, i) => (
          <g key={i}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(v)} y2={y(v)} stroke="var(--line)" strokeWidth={1} />
            <text x={PAD.l - 6} y={y(v) + 3.5} textAnchor="end" fontSize={10} fill="var(--fg-faint)">
              {num(Math.round(v))}
            </text>
          </g>
        ))}
        {days.map((d, i) => {
          const lo = lower.key(d);
          const up = upper.key(d);
          const x = PAD.l + i * slot + (slot - barW) / 2;
          const loH = (lo / max) * plotH;
          const upH = (up / max) * plotH;
          const base = PAD.t + plotH;
          return (
            <g key={d.date}>
              {lo > 0 ? <rect x={x} y={base - loH} width={barW} height={loH} rx={2} fill={lower.color} /> : null}
              {up > 0 ? <rect x={x} y={base - loH - upH - 2} width={barW} height={Math.max(1, upH)} rx={2} fill={upper.color} /> : null}
              <rect
                x={PAD.l + i * slot}
                y={PAD.t}
                width={slot}
                height={plotH}
                fill={hover === i ? "var(--fg)" : "transparent"}
                opacity={hover === i ? 0.06 : 0}
                onMouseEnter={() => setHover(i)}
                onMouseLeave={() => setHover(null)}
              />
            </g>
          );
        })}
        <line x1={PAD.l} x2={W - PAD.r} y1={PAD.t + plotH} y2={PAD.t + plotH} stroke="var(--line-strong)" strokeWidth={1} />
        {days.map((d, i) =>
          i === 0 || i === days.length - 1 || i === Math.floor(days.length / 2) ? (
            <text key={d.date} x={PAD.l + i * slot + slot / 2} y={H - 6} textAnchor="middle" fontSize={10} fill="var(--fg-faint)">
              {tick(d.date)}
            </text>
          ) : null,
        )}
      </svg>
      {!anyData ? <div className="absolute inset-0 flex items-center justify-center text-xs text-fg-muted">{empty}</div> : null}
      <div className="mt-1 flex items-center gap-4 text-xs">
        <span className="inline-flex items-center gap-1.5">
          <span className="inline-block w-2.5 h-2.5 rounded-sm border" style={{ background: lower.color, borderColor: "var(--line-strong)" }} /> {lower.label}
        </span>
        <span className="inline-flex items-center gap-1.5">
          <span className="inline-block w-2.5 h-2.5 rounded-sm border" style={{ background: upper.color, borderColor: "var(--line-strong)" }} /> {upper.label}
        </span>
        {h ? (
          <span className="ml-auto tnum text-fg-muted">
            {h.date}: {num(lower.key(h))} {lower.label.toLowerCase()} · {num(upper.key(h))} {upper.label.toLowerCase()} {unit}
          </span>
        ) : null}
      </div>
    </div>
  );
}

/** Cores over time. A rate, so dots for what was measured and a line only where two measured days touch. */
function CoresChart({ days, cores }: { days: DayBucket[]; cores: number | null }) {
  const [hover, setHover] = useState<number | null>(null);
  const W = 720;
  const H = 150;
  const PAD = { l: 46, r: 8, t: 10, b: 22 };
  const plotW = W - PAD.l - PAD.r;
  const plotH = H - PAD.t - PAD.b;
  const max = Math.max(1, cores ?? 0, ...days.map((d) => d.machine_cores ?? 0));
  const slot = plotW / Math.max(1, days.length);
  const x = (i: number) => PAD.l + i * slot + slot / 2;
  const y = (v: number) => PAD.t + plotH - (v / max) * plotH;
  const pts = days.map((d, i) => ({ i, v: d.endpoint_cores, m: d.machine_cores, date: d.date })).filter((p) => p.v !== null);
  const h = hover !== null ? days[hover] : undefined;
  return (
    <div>
      <svg viewBox={`0 0 ${W} ${H}`} className="w-full h-auto" role="img" aria-label="cores burned per day">
        {cores ? (
          <g>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(cores)} y2={y(cores)} stroke="var(--line-strong)" strokeWidth={1} strokeDasharray="3 3" />
            <text x={W - PAD.r} y={y(cores) + 13} textAnchor="end" fontSize={10} fill="var(--fg-faint)">
              {cores} cores in this machine
            </text>
          </g>
        ) : null}
        {pts.map((p, k) => {
          const prev = pts[k - 1];
          return prev && prev.i === p.i - 1 ? <line key={`l${p.i}`} x1={x(prev.i)} y1={y(prev.v as number)} x2={x(p.i)} y2={y(p.v as number)} stroke={A} strokeWidth={2} /> : null;
        })}
        {pts.map((p) => (
          <circle key={p.i} cx={x(p.i)} cy={y(p.v as number)} r={4} fill={A} />
        ))}
        {days.map((_, i) => (
          <rect key={i} x={PAD.l + i * slot} y={PAD.t} width={slot} height={plotH} fill="transparent" onMouseEnter={() => setHover(i)} onMouseLeave={() => setHover(null)} />
        ))}
        <line x1={PAD.l} x2={W - PAD.r} y1={PAD.t + plotH} y2={PAD.t + plotH} stroke="var(--line-strong)" strokeWidth={1} />
        {[0, max].map((v, i) => (
          <text key={i} x={PAD.l - 6} y={y(v) + 3.5} textAnchor="end" fontSize={10} fill="var(--fg-faint)">
            {v.toFixed(0)}
          </text>
        ))}
      </svg>
      <div className="mt-1 flex items-center gap-4 text-xs">
        <span className="inline-flex items-center gap-1.5">
          <span className="inline-block w-2.5 h-2.5 rounded-full" style={{ background: A }} /> cores burned by the endpoint
        </span>
        {h && h.endpoint_cores !== null ? (
          <span className="ml-auto tnum text-fg-muted">
            {h.date}: {h.endpoint_cores.toFixed(1)} cores{h.machine_cores !== null ? ` · ${h.machine_cores.toFixed(1)} machine-wide` : ""}
          </span>
        ) : null}
      </div>
    </div>
  );
}

function savedPct(t: UsageTotals): number {
  return t.full > 0 ? Math.round(((t.full - t.returned) / t.full) * 100) : 0;
}

export function UsageScreen({ route }: { route: Route }) {
  const days = Number(route.query.get("days")) || 30;
  const u = useAsync(() => api.usage(days), [days]);
  const [table, setTable] = useState(false);
  useShortcuts("usage", [{ keys: "r", label: "Refresh", run: () => u.reload() }]);

  const totals = useMemo(() => {
    const t = Object.values(u.data?.inference.tasks ?? {});
    const tok = t.reduce((a, x) => a + x.prompt_tokens + x.completion_tokens, 0);
    const ms = t.reduce((a, x) => a + x.elapsed_ms, 0);
    const cached = t.reduce((a, x) => a + x.cached_prompt_tokens, 0);
    const prompt = t.reduce((a, x) => a + x.prompt_tokens, 0);
    return { tok, ms, cached, prompt, tps: ms > 0 ? tok / (ms / 1000) : null };
  }, [u.data]);

  if (u.error) return <div className="p-6"><ErrorBanner error={u.error} onRetry={u.reload} /></div>;
  const d = u.data;
  if (!d) return <Loading label="reading the ledgers" />;
  const load: LoadSummary = d.load;

  return (
    <div className="p-6 space-y-6 overflow-auto scroll-thin h-full">
      <div className="flex items-center gap-3 flex-wrap">
        <h1 className="text-lg font-semibold">Usage</h1>
        <span className="text-xs text-fg-muted">what retrieval saved, and what the local model charged for it</span>
        <div className="ml-auto flex items-center gap-1.5">
          {RANGES.map((r) => (
            <a key={r} href={href("usage", null, { days: r })} className={`btn btn-sm ${r === days ? "btn-primary" : ""}`}>
              {r} d
            </a>
          ))}
          <button className="btn btn-sm" onClick={() => setTable((t) => !t)} aria-pressed={table}>
            {table ? "charts" : "table"}
          </button>
          <button className="btn btn-sm" onClick={() => u.reload()} disabled={u.loading}>
            refresh
          </button>
        </div>
      </div>

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <Stat label="recall saved" value={`${savedPct(d.retrieval.recall)}%`} sub={`${num(d.retrieval.recall.returned)} of ${num(d.retrieval.recall.full)} tokens read`} />
        <Stat label="find saved" value={`${savedPct(d.retrieval.find)}%`} sub={`${num(d.retrieval.find.returned)} of ${num(d.retrieval.find.full)} lines read`} />
        <Stat label="model throughput" value={totals.tps === null ? "—" : `${Math.round(totals.tps)} tok/s`} sub={`${num(totals.tok)} tokens over ${Math.round(totals.ms / 1000)} s of waiting`} />
        <Stat
          label="endpoint cpu"
          value={load.endpoint_cores_avg === null ? "not attributed" : `${load.endpoint_cores_avg.toFixed(1)} cores`}
          sub={load.endpoint_cores_avg === null ? "set inference.load_cgroup to attribute exactly" : `average per call${load.cores_total ? ` · ${Math.round((load.endpoint_cores_avg / load.cores_total) * 100)}% of ${load.cores_total}` : ""}`}
          tone={load.endpoint_cores_avg === null ? "warn" : undefined}
        />
      </div>

      {table ? (
        <Section title={`Day by day · ${days} days`}>
          <div className="overflow-x-auto scroll-thin">
            <table className="w-full text-xs tnum">
              <thead className="text-fg-muted text-left">
                <tr>
                  {["day", "recalls", "tokens returned", "tokens in full", "finds", "calls", "prompt", "cached", "answer", "cores"].map((h) => (
                    <th key={h} className="font-medium py-1 pr-4 whitespace-nowrap">{h}</th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {d.days.filter((x) => x.recall.ops || x.find.ops || x.calls).map((x) => (
                  <tr key={x.date} className="border-t">
                    <td className="py-1 pr-4 whitespace-nowrap">{x.date}</td>
                    <td className="py-1 pr-4">{num(x.recall.ops)}</td>
                    <td className="py-1 pr-4">{num(x.recall.returned)}</td>
                    <td className="py-1 pr-4">{num(x.recall.full)}</td>
                    <td className="py-1 pr-4">{num(x.find.ops)}</td>
                    <td className="py-1 pr-4">{num(x.calls)}</td>
                    <td className="py-1 pr-4">{num(x.prompt_tokens)}</td>
                    <td className="py-1 pr-4">{num(x.cached_prompt_tokens)}</td>
                    <td className="py-1 pr-4">{num(x.completion_tokens)}</td>
                    <td className="py-1 pr-4">{x.endpoint_cores === null ? "—" : x.endpoint_cores.toFixed(1)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="mt-2 text-2xs text-fg-faint">Days with nothing recorded are left out of this table and kept in the charts, where the axis has to stay continuous.</p>
        </Section>
      ) : (
        <div className="space-y-5">
          <Section title="Context tokens per day" aside={<span className="text-2xs">recall · {days} days</span>}>
            <StackedDays
              days={d.days}
              lower={{ label: "returned", key: (x) => x.recall.returned, color: A }}
              upper={{ label: "never read", key: (x) => Math.max(0, x.recall.full - x.recall.returned), color: "var(--surface-3)" }}
              unit="tokens"
              empty="No recall recorded in this range."
            />
            <p className="mt-2 text-2xs text-fg-faint">
              The bar is what the notes behind the hits hold in full; the coloured part is what was handed over. The rest is the ceiling that reading them whole would have cost — a saving only where reading them whole was the real alternative.
            </p>
          </Section>

          <Section title="Model prompt tokens per day" aside={<span className="text-2xs">{num(totals.cached)} of {num(totals.prompt)} from cache</span>}>
            <StackedDays
              days={d.days}
              lower={{ label: "from cache", key: (x) => x.cached_prompt_tokens, color: A }}
              upper={{ label: "processed", key: (x) => Math.max(0, x.prompt_tokens - x.cached_prompt_tokens), color: B }}
              unit="tokens"
              empty="No model call recorded in this range."
            />
            <p className="mt-2 text-2xs text-fg-faint">
              Cache hits are prompt tokens the endpoint did not process again. On a local model that buys time, not money — nothing here was billed and nothing left this machine. Calls made before the cache figure was recorded report none, which makes the cached share a floor.
            </p>
          </Section>

          <Section title="CPU during model calls" aside={load.calls_without_attribution ? <Pill tone="warn">{num(load.calls_without_attribution)} calls unattributed</Pill> : null}>
            <CoresChart days={d.days} cores={load.cores_total} />
            <p className="mt-2 text-2xs text-fg-faint">
              Cores the endpoint's own cgroup burned, averaged per day. Exact attribution, and it exists only because <code>inference.load_cgroup</code> names that cgroup; without it the page falls back to machine-wide figures and says so. The dashed line is what this machine has.
            </p>
          </Section>
        </div>
      )}

      <div className="grid gap-5 xl:grid-cols-2">
        <Section title="What the local model cost" aside={d.inference.last ? <span className="text-2xs">last call {relTime(d.inference.last)}</span> : <Pill>no calls yet</Pill>}>
          {Object.keys(d.inference.tasks).length === 0 ? (
            <p className="text-sm text-fg-muted">No chat completion recorded. Without a configured model the contradiction check, ring proposals and session summaries stay off, and recall says so in its caveats.</p>
          ) : (
            <ul className="space-y-2.5">
              {Object.entries(d.inference.tasks).map(([task, t]: [string, TaskUsage]) => (
                <li key={task} className="panel px-4 py-3">
                  <div className="flex items-baseline justify-between gap-3 flex-wrap">
                    <code className="text-sm">{task}</code>
                    <span className="text-xs text-fg-faint tnum">
                      {num(t.calls)} calls{t.failed ? <span className="text-warn"> · {num(t.failed)} failed</span> : null} · {Math.round(t.elapsed_ms / Math.max(1, t.calls) / 1000)} s each
                    </span>
                  </div>
                  <div className="mt-2 grid grid-cols-3 gap-3 text-sm tnum">
                    <div>
                      <div className="label">prompt</div>
                      <div className="mt-0.5">{num(t.prompt_tokens)}</div>
                    </div>
                    <div>
                      <div className="label">from cache</div>
                      <div className="mt-0.5">{num(t.cached_prompt_tokens)}</div>
                    </div>
                    <div>
                      <div className="label">answer</div>
                      <div className="mt-0.5">{num(t.completion_tokens)}</div>
                    </div>
                  </div>
                  {t.calls_without_cache_report || t.calls_without_counts ? (
                    <div className="mt-1.5 text-2xs text-fg-faint">
                      {t.calls_without_cache_report ? `${num(t.calls_without_cache_report)} calls reported no cache figure. ` : ""}
                      {t.calls_without_counts ? `${num(t.calls_without_counts)} calls reported no counts at all.` : ""}
                    </div>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </Section>

        <Section title="Held in memory" aside={d.loaded_models === null ? <Pill>endpoint does not say</Pill> : null}>
          {d.loaded_models === null ? (
            <p className="text-sm text-fg-muted">
              This endpoint does not answer <code>/api/ps</code>, so what it holds in memory is unknown. Only Ollama answers it; the two OpenAI routes carry no such field, and guessing it from the outside cannot tell weights from page cache.
            </p>
          ) : d.loaded_models.length === 0 ? (
            <p className="text-sm text-fg-muted">Nothing loaded right now. The next call pays the load time before it answers.</p>
          ) : (
            <ul className="space-y-2">
              {d.loaded_models.map((m: LoadedModel) => (
                <li key={m.name} className="panel px-3 py-2.5">
                  <div className="flex items-baseline justify-between gap-3 flex-wrap">
                    <code className="text-sm">{m.name}</code>
                    <span className="text-xs text-fg-faint tnum">
                      {m.parameter_size ?? "?"} · {m.quantization_level ?? "?"} · ctx {num(m.context_length)}
                    </span>
                  </div>
                  <div className="mt-1 text-xs tnum text-fg-muted">
                    {bytes(m.size)} resident · {m.size_vram === 0 ? "none in VRAM (CPU only)" : `${bytes(m.size_vram)} in VRAM`}
                    {m.expires_at ? <> · unloaded {relTime(m.expires_at)}</> : null}
                  </div>
                </li>
              ))}
            </ul>
          )}
          {load.last ? (
            <div className="mt-3 text-2xs text-fg-faint tnum">
              last call: {load.last.task}, {(load.last.wall_ms / 1000).toFixed(1)} s
              {load.last.endpoint_cores !== null ? ` · ${load.last.endpoint_cores.toFixed(1)} cores` : ""}
              {load.last.endpoint_mem_bytes !== null ? ` · ${bytes(load.last.endpoint_mem_bytes)} resident, peak ${bytes(load.last.endpoint_mem_peak_bytes)}` : ""}
            </div>
          ) : null}
        </Section>
      </div>

      <div className="text-2xs text-fg-faint">
        Ledgers: <code>usage.jsonl</code> and <code>load.jsonl</code> in the store, one line per operation; model calls come from the audit log. Days are UTC.
        {d.retrieval.unreadable_rows ? ` ${num(d.retrieval.unreadable_rows)} ledger rows could not be parsed and are left out.` : ""}{" "}
        <a className="link" href={href("status")} onClick={() => navigate(href("status"))}>store health is on Status</a>.
      </div>
    </div>
  );
}
