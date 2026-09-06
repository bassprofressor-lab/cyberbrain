import { useMemo, useState } from "react";
import { api, type DayBucket, type LoadedModel, type LoadSummary, type TaskUsage, type UsageTotals } from "@/api/client";
import { ErrorBanner, Loading, Pill, Section, Stat } from "@/components/ui";
import { bytes, dec, num, relTime } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { href, navigate, type Route } from "@/lib/router";
import { useWidth } from "@/lib/useWidth";
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
 * An axis that reads in round numbers. The step is 1, 2 or 5 x 10^n, so the ceiling is a number
 * a person recognises (400,000, not 353,862) and the grid does not re-label itself every time a
 * single new day nudges the maximum.
 */
function niceScale(max: number, steps = 2): { max: number; ticks: number[] } {
  if (!(max > 0)) return { max: 1, ticks: [0, 1] };
  const rough = max / steps;
  const mag = 10 ** Math.floor(Math.log10(rough));
  // Everything on this axis is a count, so the step never goes below 1: a store with a
  // single recorded day used to draw ticks 0, 0.5, 1 and label the last two both "1".
  const step = Math.max(1, [1, 2, 5, 10].map((m) => m * mag).find((c) => c >= rough - 1e-9) ?? 10 * mag);
  const top = Math.ceil(max / step - 1e-9) * step;
  const ticks: number[] = [];
  for (let v = 0; v <= top + step / 2; v += step) ticks.push(v);
  return { max: top, ticks };
}

/**
 * Stacked daily bars. Stacked because both pairs here are part-to-whole — what was read of
 * what there was, what came from cache of what was sent — and a part shown beside its whole
 * invites the reader to add them up a second time.
 */
function StackedDays({ days, lower, upper, unit, empty }: { days: DayBucket[]; lower: Series; upper: Series; unit: string; empty: string }) {
  const t = useT();
  const [hover, setHover] = useState<number | null>(null);
  const [wrap, W] = useWidth<HTMLDivElement>(720);
  const H = 140;
  const PAD = { l: 52, r: 8, t: 10, b: 22 };
  const plotW = W - PAD.l - PAD.r;
  const plotH = H - PAD.t - PAD.b;
  const scale = niceScale(Math.max(1, ...days.map((d) => lower.key(d) + upper.key(d))));
  const max = scale.max;
  const slot = plotW / Math.max(1, days.length);
  const barW = Math.max(2, Math.min(18, slot - 3));
  const y = (v: number) => PAD.t + plotH - (v / max) * plotH;
  const gridAt = scale.ticks;
  const anyData = days.some((d) => lower.key(d) + upper.key(d) > 0);
  const h = hover !== null ? days[hover] : undefined;

  return (
    <div className="relative" ref={wrap}>
      <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} className="block" role="img" aria-label={t.usage.chartAria(lower.label, upper.label)}>
        {gridAt.map((v, i) => (
          <g key={i}>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(v)} y2={y(v)} stroke="var(--line)" strokeWidth={1} />
            <text x={PAD.l - 6} y={y(v) + 3.5} textAnchor="end" fontSize={11} fill="var(--fg-muted)">
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
            <text key={d.date} x={PAD.l + i * slot + slot / 2} y={H - 6} textAnchor="middle" fontSize={11} fill="var(--fg-muted)">
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
            {t.usage.hoverDay(h.date, lower.key(h), lower.label, upper.key(h), upper.label, unit)}
          </span>
        ) : null}
      </div>
    </div>
  );
}

/** Cores over time. A rate, so dots for what was measured and a line only where two measured days touch. */
function CoresChart({ days, cores }: { days: DayBucket[]; cores: number | null }) {
  const t = useT();
  const [hover, setHover] = useState<number | null>(null);
  const [wrap, W] = useWidth<HTMLDivElement>(720);
  const H = 110;
  const PAD = { l: 52, r: 8, t: 10, b: 22 };
  const plotW = W - PAD.l - PAD.r;
  const plotH = H - PAD.t - PAD.b;
  const max = Math.max(1, cores ?? 0, ...days.map((d) => d.machine_cores ?? 0));
  const slot = plotW / Math.max(1, days.length);
  const x = (i: number) => PAD.l + i * slot + slot / 2;
  const y = (v: number) => PAD.t + plotH - (v / max) * plotH;
  const pts = days.map((d, i) => ({ i, v: d.endpoint_cores, m: d.machine_cores, date: d.date })).filter((p) => p.v !== null);
  const h = hover !== null ? days[hover] : undefined;
  return (
    <div ref={wrap}>
      <svg width={W} height={H} viewBox={`0 0 ${W} ${H}`} className="block" role="img" aria-label={t.usage.coresAria}>
        {cores ? (
          <g>
            <line x1={PAD.l} x2={W - PAD.r} y1={y(cores)} y2={y(cores)} stroke="var(--line-strong)" strokeWidth={1} strokeDasharray="3 3" />
            <text x={W - PAD.r} y={y(cores) + 13} textAnchor="end" fontSize={11} fill="var(--fg-muted)">
              {t.usage.coresInMachine(cores)}
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
          <text key={i} x={PAD.l - 6} y={y(v) + 3.5} textAnchor="end" fontSize={11} fill="var(--fg-muted)">
            {dec(v, 0)}
          </text>
        ))}
      </svg>
      <div className="mt-1 flex items-center gap-4 text-xs">
        <span className="inline-flex items-center gap-1.5">
          <span className="inline-block w-2.5 h-2.5 rounded-full" style={{ background: A }} /> {t.usage.coresLegend}
        </span>
        {h && h.endpoint_cores !== null ? (
          <span className="ml-auto tnum text-fg-muted">
            {t.usage.hoverCores(h.date, dec(h.endpoint_cores), h.machine_cores !== null ? dec(h.machine_cores) : null)}
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
  const t = useT();
  const days = Number(route.query.get("days")) || 30;
  const u = useAsync(() => api.usage(days), [days]);
  const [table, setTable] = useState(false);
  useShortcuts("usage", [{ keys: "r", label: t.usage.keys.refresh, run: () => u.reload() }], [t]);

  const totals = useMemo(() => {
    const tasks = Object.values(u.data?.inference.tasks ?? {});
    const tok = tasks.reduce((a, x) => a + x.prompt_tokens + x.completion_tokens, 0);
    const ms = tasks.reduce((a, x) => a + x.elapsed_ms, 0);
    const cached = tasks.reduce((a, x) => a + x.cached_prompt_tokens, 0);
    const prompt = tasks.reduce((a, x) => a + x.prompt_tokens, 0);
    return { tok, ms, cached, prompt, tps: ms > 0 ? tok / (ms / 1000) : null };
  }, [u.data]);

  if (u.error) return <div className="p-6"><ErrorBanner error={u.error} onRetry={u.reload} /></div>;
  const d = u.data;
  if (!d) return <Loading label={t.usage.loading} />;
  const load: LoadSummary = d.load;

  return (
    <div className="p-6 space-y-6 overflow-auto scroll-thin h-full">
      <div className="flex items-center gap-3 flex-wrap">
        <h1 className="text-lg font-semibold">{t.usage.title}</h1>
        <span className="text-xs text-fg-muted">{t.usage.subtitle}</span>
        <div className="ml-auto flex items-center gap-1.5">
          {RANGES.map((r) => (
            <a key={r} href={href("usage", null, { days: r })} className={`btn btn-sm ${r === days ? "btn-primary" : ""}`}>
              {t.common.dayShort(r)}
            </a>
          ))}
          <button className="btn btn-sm" onClick={() => setTable((v) => !v)} aria-pressed={table}>
            {table ? t.usage.charts : t.usage.table}
          </button>
          <button className="btn btn-sm" onClick={() => u.reload()} disabled={u.loading}>
            {t.common.refresh}
          </button>
        </div>
      </div>

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <Stat label={t.usage.stat.recallSaved} value={`${savedPct(d.retrieval.recall)}%`} sub={t.usage.stat.recallSavedSub(num(d.retrieval.recall.returned), num(d.retrieval.recall.full))} />
        <Stat label={t.usage.stat.findSaved} value={`${savedPct(d.retrieval.find)}%`} sub={t.usage.stat.findSavedSub(num(d.retrieval.find.returned), num(d.retrieval.find.full))} />
        <Stat label={t.usage.stat.throughput} value={totals.tps === null ? "—" : t.usage.stat.throughputValue(Math.round(totals.tps))} sub={t.usage.stat.throughputSub(num(totals.tok), Math.round(totals.ms / 1000))} />
        <Stat
          label={t.usage.stat.cpu}
          value={load.endpoint_cores_avg === null ? t.usage.stat.cpuNone : t.usage.stat.cpuValue(dec(load.endpoint_cores_avg))}
          sub={load.endpoint_cores_avg === null ? t.usage.stat.cpuNoneSub : `${t.usage.stat.cpuSub}${load.cores_total ? t.usage.stat.cpuShare(Math.round((load.endpoint_cores_avg / load.cores_total) * 100), load.cores_total) : ""}`}
          tone={load.endpoint_cores_avg === null ? "warn" : undefined}
        />
      </div>

      {table ? (
        <Section title={t.usage.dayByDay(days)}>
          <div className="overflow-x-auto scroll-thin">
            <table className="w-full text-xs tnum">
              <thead className="text-fg-muted text-left">
                <tr>
                  {t.usage.columns.map((h) => (
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
                    <td className="py-1 pr-4">{x.endpoint_cores === null ? "—" : dec(x.endpoint_cores)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <p className="mt-2 text-2xs text-fg-faint">{t.usage.tableNote}</p>
        </Section>
      ) : (
        <div className="space-y-5">
          <Section title={t.usage.contextChart} aside={<span className="text-2xs">{t.usage.contextAside(days)}</span>}>
            <StackedDays
              days={d.days}
              lower={{ label: t.usage.returned, key: (x) => x.recall.returned, color: A }}
              upper={{ label: t.usage.neverRead, key: (x) => Math.max(0, x.recall.full - x.recall.returned), color: "var(--surface-3)" }}
              unit={t.common.tokens}
              empty={t.usage.emptyRecall}
            />
            <p className="mt-2 text-2xs text-fg-faint">{t.usage.contextNote}</p>
          </Section>

          <Section title={t.usage.promptChart} aside={<span className="text-2xs">{t.usage.promptAside(num(totals.cached), num(totals.prompt))}</span>}>
            <StackedDays
              days={d.days}
              lower={{ label: t.usage.fromCache, key: (x) => x.cached_prompt_tokens, color: A }}
              upper={{ label: t.usage.processed, key: (x) => Math.max(0, x.prompt_tokens - x.cached_prompt_tokens), color: B }}
              unit={t.common.tokens}
              empty={t.usage.emptyCalls}
            />
            <p className="mt-2 text-2xs text-fg-faint">{t.usage.promptNote}</p>
          </Section>

          <Section title={t.usage.cpuChart} aside={load.calls_without_attribution ? <Pill tone="warn">{t.usage.unattributed(num(load.calls_without_attribution))}</Pill> : null}>
            <CoresChart days={d.days} cores={load.cores_total} />
            <p className="mt-2 text-2xs text-fg-faint">{t.usage.cpuNote}</p>
          </Section>
        </div>
      )}

      <div className="grid gap-5 xl:grid-cols-2">
        <Section title={t.usage.cost.title} aside={d.inference.last ? <span className="text-2xs">{t.usage.cost.lastCall(relTime(d.inference.last))}</span> : <Pill>{t.usage.cost.noCalls}</Pill>}>
          {Object.keys(d.inference.tasks).length === 0 ? (
            <p className="text-sm text-fg-muted">{t.usage.cost.none}</p>
          ) : (
            <ul className="space-y-2.5">
              {Object.entries(d.inference.tasks).map(([task, tu]: [string, TaskUsage]) => (
                <li key={task} className="panel px-4 py-3">
                  <div className="flex items-baseline justify-between gap-3 flex-wrap">
                    <code className="text-sm">{task}</code>
                    <span className="text-xs text-fg-faint tnum">
                      {t.usage.cost.calls(num(tu.calls))}{tu.failed ? <span className="text-warn">{t.usage.cost.failed(num(tu.failed))}</span> : null}{t.usage.cost.each(Math.round(tu.elapsed_ms / Math.max(1, tu.calls) / 1000))}
                    </span>
                  </div>
                  <div className="mt-2 grid grid-cols-3 gap-3 text-sm tnum">
                    <div>
                      <div className="label">{t.usage.cost.prompt}</div>
                      <div className="mt-0.5">{num(tu.prompt_tokens)}</div>
                    </div>
                    <div>
                      <div className="label">{t.usage.cost.fromCache}</div>
                      <div className="mt-0.5">{num(tu.cached_prompt_tokens)}</div>
                    </div>
                    <div>
                      <div className="label">{t.usage.cost.answer}</div>
                      <div className="mt-0.5">{num(tu.completion_tokens)}</div>
                    </div>
                  </div>
                  {tu.calls_without_cache_report || tu.calls_without_counts ? (
                    <div className="mt-1.5 text-2xs text-fg-faint">
                      {tu.calls_without_cache_report ? t.usage.cost.noCacheFigure(num(tu.calls_without_cache_report)) : ""}
                      {tu.calls_without_counts ? t.usage.cost.noCounts(num(tu.calls_without_counts)) : ""}
                    </div>
                  ) : null}
                </li>
              ))}
            </ul>
          )}
        </Section>

        <Section title={t.usage.memory.title} aside={d.loaded_models === null ? <Pill>{t.usage.memory.endpointSilent}</Pill> : null}>
          {d.loaded_models === null ? (
            <p className="text-sm text-fg-muted">{t.usage.memory.noApi}</p>
          ) : d.loaded_models.length === 0 ? (
            <p className="text-sm text-fg-muted">{t.usage.memory.nothing}</p>
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
                    {t.usage.memory.resident(bytes(m.size))} · {m.size_vram === 0 ? t.usage.memory.noVram : t.usage.memory.vram(bytes(m.size_vram))}
                    {m.expires_at ? <> · {t.usage.memory.unloaded(relTime(m.expires_at))}</> : null}
                  </div>
                </li>
              ))}
            </ul>
          )}
          {load.last ? (
            <div className="mt-3 text-2xs text-fg-faint tnum">
              {t.usage.memory.lastCall(load.last.task, dec(load.last.wall_ms / 1000))}
              {load.last.endpoint_cores !== null ? t.usage.memory.lastCores(dec(load.last.endpoint_cores)) : ""}
              {load.last.endpoint_mem_bytes !== null ? t.usage.memory.lastMem(bytes(load.last.endpoint_mem_bytes), bytes(load.last.endpoint_mem_peak_bytes)) : ""}
            </div>
          ) : null}
        </Section>
      </div>

      <div className="text-2xs text-fg-faint">
        {t.usage.footer.ledgers}
        {d.retrieval.unreadable_rows ? t.usage.footer.unreadable(num(d.retrieval.unreadable_rows)) : ""}{" "}
        <a className="link" href={href("status")} onClick={() => navigate(href("status"))}>{t.usage.footer.storeHealth}</a>.
      </div>
    </div>
  );
}
