import { useState, type ReactNode } from "react";
import { api, type DoctorFinding, type DoctorReport, type InferenceBackend, type Ring, type ScanReport } from "@/api/client";
import { RingGlyph } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Card, MetricCard, StateMark, StatusBadge, worst, type State } from "@/components/design";
import { Icon } from "@/components/icons";
import { Dot, ErrorBanner, Kbd, Loading, Pill } from "@/components/ui";
import { absTime, bytes, num, relTime } from "@/lib/format";
import { type Dict, useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { href, navigate } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

const BACKEND_LABEL: Record<InferenceBackend, string> = {
  ollama: "Ollama",
  "lm-studio": "LM Studio",
  "llama-cpp": "llama.cpp",
  "vllm": "vLLM",
  "nvidia-pair": "NVIDIA PAIR",
  unknown: "unknown",
};

/**
 * Two checks that must not read the same. A dangling link names a note nobody has written
 * yet: wait for it. An unresolvable link names something that can never be a note: no
 * amount of waiting resolves it.
 */
function checkLabel(f: DoctorFinding, t: Dict): { label: string; tone: "ok" | "warn" | "danger" | "neutral"; hint: string } {
  if (f.check === "dangling-link") return { label: t.status.doctor.danglingIntent, tone: "neutral", hint: t.status.doctor.danglingIntentHint };
  if (f.check === "unresolvable-links" || f.check === "unresolvable-link") return { label: t.status.doctor.unresolvable, tone: "warn", hint: t.status.doctor.unresolvableHint };
  const tone = f.severity === "error" ? "danger" : f.severity === "warn" ? "warn" : "neutral";
  return { label: f.check, tone, hint: t.status.doctor.checkHint(f.check) };
}

export function StatusScreen() {
  const t = useT();
  const s = useAsync(() => api.status(), []);
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [scan, setScan] = useState<ScanReport | null>(null);
  const [busy, setBusy] = useState<"" | "doctor" | "scan" | "full">("");
  const toast = useToast();

  const runDoctor = async () => {
    setBusy("doctor");
    try {
      setDoctor(await api.doctor());
    } catch (e) {
      toast(toApiError(e).message, "err");
    } finally {
      setBusy("");
    }
  };
  const runScan = async (full: boolean) => {
    setBusy(full ? "full" : "scan");
    try {
      const r = await api.scan(full);
      setScan(r);
      toast(t.status.scanToast({ changed: r.changed, added: r.added, removed: r.removed, ms: r.elapsed_ms }));
      s.reload();
    } catch (e) {
      toast(toApiError(e).message, "err");
    } finally {
      setBusy("");
    }
  };

  useShortcuts(
    "status",
    [
      { keys: "r", label: t.status.keys.refresh, run: () => s.reload() },
      { keys: "d", label: t.status.keys.doctor, run: runDoctor },
    ],
    [t],
  );

  if (s.error) return <div className="p-5"><ErrorBanner error={s.error} onRetry={s.reload} /></div>;
  const d = s.data;
  if (!d) return <Loading label={t.status.loading} />;

  const dd = t.design.status;
  const capPct = Math.round((d.store.resident_cap.used / d.store.resident_cap.tokens) * 100);
  const maxRingBlocks = Math.max(1, ...d.store.rings.map((r) => r.blocks));
  const endpointTone = d.inference.endpoint_class === "public" ? (d.inference.allow_public_endpoint ? "warn" : "danger") : "ok";
  const backendLabel = (b: InferenceBackend) => (b === "unknown" ? t.status.stat.backendUnknown : BACKEND_LABEL[b]);

  // --- the states, each from a field the API really sends -------------------------------
  // index: a broken FTS5 is an error; stale notes, orphan vectors and links without a target
  // are warnings; nothing else is.
  const indexWarnings: string[] = [];
  if (d.index.stale_notes > 0) indexWarnings.push(dd.card.stale(d.index.stale_notes));
  if (d.index.orphan_vectors > 0) indexWarnings.push(dd.card.orphans(d.index.orphan_vectors));
  // Links without a target are information: most are intent (a note somebody will write).
  // The status call cannot tell them from unresolvable ones; `doctor` can, and only those warn.
  const unresolvable = doctor ? doctor.findings.filter((f) => f.check === "unresolvable-links" || f.check === "unresolvable-link").length : 0;
  if (unresolvable > 0) indexWarnings.push(dd.card.unresolvable(unresolvable));
  const indexState: State = !d.index.fts_ok ? "danger" : indexWarnings.length ? "warn" : "ok";

  // embedding: not loaded is "not set up" (optional) unless the store holds model data that
  // was not loaded, which is worth a warning; vectors from another model are an error.
  const embMismatch = d.embedding.loaded && d.embedding.matches_index === false;
  const embState: State = embMismatch ? "danger" : d.embedding.loaded ? "ok" : d.store.models_bytes > 0 ? "warn" : "idle";

  // inference: not configured is "not set up". Configured, it is an error when the address
  // is refused or the probe failed; a public address that was allowed is a warning; a probe
  // that did not run is "not checked yet", which is neither working nor broken.
  const infConfigured = d.inference.configured;
  const infUnchecked = infConfigured && endpointTone === "ok" && d.inference.reachable === null;
  const infState: State = !infConfigured ? "idle" : endpointTone === "danger" || d.inference.reachable === false ? "danger" : endpointTone === "warn" ? "warn" : infUnchecked ? "idle" : "ok";

  const infLabel = !infConfigured ? dd.state.idle : infUnchecked ? dd.state.unchecked : d.inference.reachable === false ? dd.card.infDown : endpointTone === "danger" ? dd.card.infRefused : endpointTone === "warn" ? dd.card.infPublic : dd.card.infOk;
  const areas: Array<{ key: string; name: string; state: State; unchecked?: boolean }> = [
    { key: "store", name: dd.area.store, state: "ok" },
    { key: "index", name: dd.area.index, state: indexState },
    { key: "embedding", name: dd.area.embedding, state: embState },
    { key: "inference", name: dd.area.inference, state: infState, unchecked: infUnchecked },
  ];
  const by = (st: State, unchecked = false) => areas.filter((a) => a.state === st && !!a.unchecked === unchecked);
  const nDanger = by("danger").length;
  const nWarn = by("warn").length;
  const nIdle = by("idle").length;
  const nUnchecked = by("idle", true).length;
  const overall = worst(areas.map((a) => a.state));
  const headline = nDanger ? dd.headline.danger(nDanger) : nWarn ? dd.headline.warn(nWarn) : nIdle ? dd.headline.idle(nIdle) : nUnchecked ? dd.headline.unchecked(nUnchecked) : dd.headline.ok;
  const bannerState: State = nDanger ? "danger" : nWarn ? "warn" : nIdle || nUnchecked ? "idle" : "ok";
  const bannerLines: Array<[string, string]> = [];
  if (nDanger) bannerLines.push([dd.cat.danger, by("danger").map((a) => a.name).join(", ")]);
  if (nWarn) bannerLines.push([dd.cat.warn, by("warn").map((a) => a.name).join(", ")]);
  if (nIdle) bannerLines.push([dd.cat.idle, by("idle").map((a) => a.name).join(", ")]);
  if (nUnchecked) bannerLines.push([dd.cat.unchecked, by("idle", true).map((a) => a.name).join(", ")]);

  const lastScan = d.index.last_scan ? relTime(d.index.last_scan) : dd.card.notChecked;
  const cap = (x: string) => x.charAt(0).toUpperCase() + x.slice(1);
  const lastFull = d.index.last_full_scan ? relTime(d.index.last_full_scan) : dd.index.notChecked;
  const lastScanRow = d.index.last_scan ? relTime(d.index.last_scan) : dd.index.notChecked;
  const idx = dd.index;

  const kv = (rows: Array<[ReactNode, ReactNode]>) => (
    <dl className="kv">
      {rows.map(([k, v], i) => (
        <div key={i}>
          <dt>{k}</dt>
          <dd>{v}</dd>
        </div>
      ))}
    </dl>
  );
  const row = (n: number, label: string, hint?: string) => (
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-0.5">
      <span className={`inline-flex items-center gap-1.5 tnum font-medium ${n > 0 ? "text-warn" : ""}`}>
        {n > 0 ? <Icon name="alert" size={15} strokeWidth={2} /> : null}
        {n}
      </span>
      {hint ? <span className="text-xs text-fg-muted">{hint}</span> : null}
      <span className="sr-only">{label}</span>
    </span>
  );

  return (
    <div className="px-4 py-5 sm:px-6 sm:py-6 space-y-4 sm:space-y-5 overflow-auto scroll-thin h-full">
      <div className="flex items-center gap-x-4 gap-y-3 flex-wrap">
        <div className="flex items-baseline gap-3 min-w-0 mr-auto">
          <h1 className="text-2xl font-semibold tracking-tight">{t.status.title}</h1>
          <span className="font-mono text-xs text-fg-muted">cyberbrain {d.version}</span>
        </div>
        <button className="search-btn order-last sm:order-none basis-full sm:basis-auto" onClick={() => navigate(href("search"))} aria-label={dd.searchAria}>
          <Icon name="search" size={18} />
          <span className="flex-1 truncate">{dd.searchPlaceholder}</span>
          <Kbd keys="Mod+k" className="shrink-0" />
        </button>
        <div className="flex flex-wrap gap-2">
          <button className="btn btn-win" onClick={() => s.reload()} disabled={s.loading} title={`${dd.btn.refreshTitle} (r)`}>
            <Icon name="refresh" size={18} />
            {dd.btn.refresh}
          </button>
          <button className="btn btn-win" onClick={runDoctor} disabled={busy !== ""} title={`${dd.btn.checkTitle} (d) · cyberbrain doctor`}>
            {busy === "doctor" ? dd.btn.checkBusy : dd.btn.check}
          </button>
          <button className="btn btn-win" onClick={() => runScan(false)} disabled={busy !== ""} title={`${dd.btn.updateTitle} · cyberbrain scan`}>
            {busy === "scan" ? dd.btn.updateBusy : dd.btn.update}
          </button>
          <button className="btn btn-win btn-ghost" onClick={() => runScan(true)} disabled={busy !== ""} title={`${t.status.btn.fullTitle} · cyberbrain scan --full`}>
            {busy === "full" ? dd.btn.rebuildBusy : dd.btn.rebuild}
          </button>
        </div>
      </div>

      <div className={`banner banner-${bannerState}`} role="status">
        <StateMark state={bannerState} />
        <div className="min-w-0">
          <div className="text-lg font-semibold leading-snug">{headline}</div>
          <div className="mt-0.5 text-sm text-fg-muted space-y-0.5">
            {bannerLines.length ? bannerLines.map(([k, v]) => (
              <div key={k}>
                <span className="font-medium text-fg">{k}:</span> {v}
              </div>
            )) : dd.okSub}
          </div>
        </div>
      </div>

      <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-4">
        <MetricCard icon="database" label={dd.area.store} value={bytes(d.store.bytes)} sub={t.status.stat.storeSub(num(d.store.notes), num(d.store.blocks), num(d.store.vectors))} />
        <MetricCard
          icon="index"
          label={dd.area.index}
          state={indexState}
          value={!d.index.fts_ok ? dd.card.indexFts : indexWarnings.length ? dd.card.indexWarnings(indexWarnings.length) : dd.card.indexFresh}
          sub={indexWarnings.length ? indexWarnings.join(" · ") : dd.card.lastScan(lastScan)}
        />
        <MetricCard
          icon="cpu"
          label={dd.area.embedding}
          state={embState}
          value={embMismatch ? dd.card.embMismatch : embState === "ok" ? dd.card.embLoaded : embState === "warn" ? dd.card.embWarn : dd.state.idle}
          sub={embMismatch ? dd.card.embMismatchSub : embState === "ok" ? `${d.embedding.model.split("/").pop()} · d${d.embedding.dim}` : embState === "warn" ? dd.card.embWarnSub(bytes(d.store.models_bytes)) : dd.card.embIdleSub}
        />
        <MetricCard
          icon="server"
          label={dd.area.inference}
          state={infState}
          value={infLabel}
          sub={!infConfigured ? dd.card.infIdleSub : [d.inference.model, d.inference.last_backend !== "unknown" ? backendLabel(d.inference.last_backend) : null].filter(Boolean).join(" · ") || dd.card.infUnchecked}
        />
      </div>

      {embMismatch ? (
        <div className="alert alert-danger" role="alert">
          <span className="font-medium text-danger">{t.status.mismatch.lead}</span> {t.status.mismatch.body(d.embedding.profile_id)}
        </div>
      ) : null}

      <div className="grid gap-4 sm:gap-5 xl:grid-cols-2">
        <Card title={t.status.store.title} icon="database" aside={<code className="text-xs truncate" title={d.store.path}>{d.store.path}</code>}>
          {kv([
            [t.status.store.notes, t.status.store.notesValue(bytes(d.store.notes_bytes))],
            [t.status.store.db, t.status.store.dbValue(bytes(d.store.db_bytes))],
            [t.status.store.models, t.status.store.modelsValue(bytes(d.store.models_bytes))],
          ])}
          <div className="mt-5">
            <div className="flex flex-wrap items-baseline justify-between gap-x-3">
              <span className="text-sm font-medium">{t.status.store.cap}</span>
              <span className="text-sm tnum text-fg-muted">{t.status.store.capValue(num(d.store.resident_cap.used), num(d.store.resident_cap.tokens), capPct)}</span>
            </div>
            <div className="mt-2 h-2 rounded-full bg-surface-3 overflow-hidden" role="progressbar" aria-valuenow={Math.min(100, capPct)} aria-valuemin={0} aria-valuemax={100}>
              <div className="h-full rounded-full" style={{ width: `${Math.min(100, capPct)}%`, background: capPct > 90 ? "var(--danger)" : capPct > 70 ? "var(--warn)" : "var(--ok)" }} />
            </div>
            <div className="mt-1.5 text-xs text-fg-muted">{t.status.store.capNote}</div>
          </div>
          <div className="mt-5">
            <div className="text-sm font-medium mb-2">{t.status.store.blocksPerRing}</div>
            <ul className="space-y-2">
              {d.store.rings.map((r) => (
                <li key={r.ring} className="grid grid-cols-[6rem_minmax(0,1fr)] sm:grid-cols-[6.5rem_minmax(0,1fr)_auto] items-center gap-x-3 gap-y-1 text-sm">
                  <a href={href("notes", null, { ring: r.ring })} className="inline-flex items-center gap-1.5 hover:underline">
                    <RingGlyph ring={r.ring as Ring} size={10} />
                    <span className="font-mono" style={{ color: `var(--ring-${r.ring})` }}>
                      r{r.ring}
                    </span>
                    <span className="text-fg-muted">{t.rings.label[r.ring]}</span>
                  </a>
                  <div className="h-2 rounded-full bg-surface-3 overflow-hidden">
                    <div className="h-full rounded-full" style={{ width: `${(r.blocks / maxRingBlocks) * 100}%`, background: `var(--ring-${r.ring})` }} />
                  </div>
                  <span className="tnum text-xs text-fg-muted sm:text-right whitespace-nowrap col-span-2 sm:col-span-1">{t.status.store.ringCounts(num(r.notes), num(r.blocks), num(r.tokens))}</span>
                </li>
              ))}
            </ul>
          </div>
        </Card>

        <Card
          title={t.status.index.title}
          icon="index"
          aside={
            <>
              <span className="hidden sm:inline">{t.status.index.schema(d.index.schema_version)}</span>
              <StatusBadge state={indexState}>{dd.state[indexState]}</StatusBadge>
            </>
          }
        >
          {kv([
            ["FTS5", d.index.fts_ok ? <StatusBadge state="ok">{t.status.index.ftsOk.toUpperCase()}</StatusBadge> : <StatusBadge state="danger">{t.status.index.ftsBroken}</StatusBadge>],
            [idx.orphans, row(d.index.orphan_vectors, idx.orphans)],
            [cap(t.status.index.lastScan), <span title={absTime(d.index.last_scan)}>{lastScanRow}</span>],
            [cap(t.status.index.lastFull), <span title={absTime(d.index.last_full_scan)}>{lastFull}</span>],
            [cap(t.status.index.stale), row(d.index.stale_notes, t.status.index.stale, t.status.index.staleHint)],
            [idx.dangling, <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-0.5"><span className="tnum font-medium">{d.index.dangling_links}</span><span className="text-xs text-fg-muted">{t.status.index.danglingHint}</span></span>],
            ...(doctor ? ([[idx.unresolvable, row(unresolvable, idx.unresolvable, t.status.doctor.unresolvableHint)]] as Array<[ReactNode, ReactNode]>) : []),
          ])}
          {scan ? (
            <div className="mt-4 panel-inset px-3 py-2.5 text-xs">
              <Pill title={scan.full ? "cyberbrain scan --full" : "cyberbrain scan"}>{scan.full ? dd.btn.rebuild : dd.btn.update}</Pill>{scan.dry_run ? <Pill className="ml-1">{t.status.index.dryRun}</Pill> : null}{" "}
              <span className="tnum">{t.status.index.scanLine({ scanned: scan.scanned, changed: scan.changed, added: scan.added, removed: scan.removed, ms: scan.elapsed_ms, blocks: num(scan.blocks_written), vectors: num(scan.vectors_written) })}</span>
              <div className="mt-1 text-fg-faint tnum">
                {t.status.index.scanDetail({ unchanged: scan.detail.unchanged, touched: scan.detail.touched_only, revectorised: scan.detail.revectorised, links: scan.detail.links_written_back })}
                {scan.detail.skipped.length ? t.status.index.scanSkipped(scan.detail.skipped.length) : ""}
                {scan.detail.oversized_blocks.length ? t.status.index.scanOversized(scan.detail.oversized_blocks.length) : ""}
                {!scan.detail.embedder.loaded ? t.status.index.scanNoEmbedder(scan.detail.embedder.reason ?? t.status.index.noReason) : ""}
              </div>
              {scan.caveats.length ? (
                <ul className="mt-1 text-fg-muted space-y-0.5">
                  {scan.caveats.map((c, i) => (
                    <li key={i}>· {c}</li>
                  ))}
                </ul>
              ) : null}
            </div>
          ) : null}
          {doctor ? (
            <div className="mt-4">
              <div className="flex items-center gap-2 text-sm flex-wrap">
                <Dot tone={doctor.ok ? "ok" : doctor.findings.some((f) => f.severity === "error") ? "danger" : "warn"} />
                <span className="font-medium">doctor {doctor.ok ? t.status.doctor.clean : doctor.findings.some((f) => f.severity === "error") ? t.status.doctor.errors : t.status.doctor.warnings}</span>
                <span className="text-xs text-fg-muted tnum">{t.status.doctor.summary(doctor.findings.length, doctor.checks_run.length, relTime(doctor.checked_at))}</span>
              </div>
              <div className="mt-1 text-xs text-fg-muted" title={doctor.checks_run.join(", ")}>
                {t.status.doctor.checks(doctor.checks_run.join(" · "))}
              </div>
              <ul className="mt-2 space-y-1 text-sm max-h-56 overflow-auto scroll-thin">
                {doctor.findings.map((f, i) => {
                  const c = checkLabel(f, t);
                  return (
                    <li key={i} className="flex gap-2">
                      <Pill tone={c.tone} className="shrink-0" title={c.hint}>
                        {c.label}
                      </Pill>
                      <span className="font-mono text-fg-muted truncate max-w-[12rem]">{f.subject}</span>
                      <span className="text-fg-muted">{f.message}</span>
                    </li>
                  );
                })}
              </ul>
            </div>
          ) : null}
        </Card>

        <Card
          title={t.status.embedding.title}
          icon="cube"
          state={embState}
          aside={
            embState === "idle" ? (
              <StatusBadge state="idle">{dd.state.idle}</StatusBadge>
            ) : embState === "warn" ? (
              <StatusBadge state="warn">{dd.card.embWarn}</StatusBadge>
            ) : d.embedding.matches_index === null ? (
              <span title={t.status.embedding.nothingToCompareTitle}>
                <StatusBadge state="ok">{t.status.embedding.nothingToCompare}</StatusBadge>
              </span>
            ) : (
              <StatusBadge state={d.embedding.matches_index ? "ok" : "danger"}>{d.embedding.matches_index ? t.status.embedding.matches : t.status.embedding.mismatch}</StatusBadge>
            )
          }
        >
          {!d.embedding.loaded ? (
            <div className="mb-4 text-sm">
              <p className={embState === "warn" ? "text-warn" : "text-fg-muted"}>{embState === "warn" ? dd.embedding.warnWhat : dd.embedding.idleWhat}</p>
              <SetupHint title={dd.embedding.howTitle} text={dd.embedding.how(d.store.path)} />
            </div>
          ) : null}
          <Details open={d.embedding.loaded} title={d.embedding.loaded ? null : dd.embedding.techTitle}>
            {kv([
              [t.status.embedding.profileId, <code className="text-xs">{d.embedding.profile_id}</code>],
              [t.status.embedding.model, <code className="text-xs">{d.embedding.model}</code>],
              [t.status.embedding.dimension, `${d.embedding.dim} · ${d.embedding.pooling}`],
              [t.status.embedding.backend, `${d.embedding.backend} (${d.embedding.backend === "static" ? t.status.embedding.backendStatic : t.status.embedding.backendCandle})`],
              [t.status.embedding.hash, d.embedding.model_hash ? <code className="text-xs break-all">{d.embedding.model_hash}</code> : <span className="text-fg-muted">{t.status.embedding.hashNone}</span>],
              [t.status.embedding.verified, d.embedding.model_verified_at ? <span title={absTime(d.embedding.model_verified_at)}>{t.status.embedding.verifiedAt(relTime(d.embedding.model_verified_at))}</span> : <span className="text-fg-muted">{t.status.embedding.verifiedNoStamp}</span>],
            ])}
          </Details>
        </Card>

        <Card
          title={t.status.inference.title}
          icon="bolt"
          state={infState}
          aside={
            <StatusBadge state={infState}>{infLabel}</StatusBadge>
          }
        >
          {infConfigured ? (
            <>
              {kv([
                [t.status.inference.baseUrl, <code className="text-xs">{d.inference.base_url}</code>],
                [
                  t.status.inference.addressClass,
                  <span className={endpointTone === "ok" ? "" : endpointTone === "warn" ? "text-warn" : "text-danger"}>
                    {d.inference.endpoint_class}
                    {d.inference.endpoint_class === "public" ? (d.inference.allow_public_endpoint ? t.status.inference.allowed : t.status.inference.refused) : t.status.inference.local}
                  </span>,
                ],
                [t.status.inference.model, d.inference.model ?? "—"],
                [
                  t.status.inference.reachable,
                  d.inference.reachable === null ? (
                    <span className="text-fg-muted">{t.status.inference.notProbed}</span>
                  ) : (
                    <span className={d.inference.reachable ? "text-ok" : "text-danger"}>
                      {d.inference.reachable ? t.common.yes : t.common.no} <span className="text-fg-muted">{t.status.inference.probedBy(relTime(d.inference.reachable_checked_at))}</span>
                    </span>
                  ),
                ],
                [t.status.inference.lastCall, d.inference.last_call ? <span title={absTime(d.inference.last_call)}>{relTime(d.inference.last_call)}</span> : <span className="text-fg-muted">{t.status.inference.noAuditRow}</span>],
              ])}
              <div className="mt-4 panel-inset px-3 py-3">
                <div className="text-sm font-medium">{t.status.inference.whichBackend}</div>
                <div className="mt-2 flex flex-wrap items-center gap-2">
                  {(["ollama", "lm-studio", "llama-cpp", "vllm", "nvidia-pair", "unknown"] as InferenceBackend[]).map((b) => (
                    <span key={b} className={`inline-flex items-center h-7 px-2.5 rounded-md border text-xs ${d.inference.last_backend === b ? "bg-accent-soft text-accent border-accent/40 font-medium" : "text-fg-muted border-line"}`}>
                      {backendLabel(b)}
                    </span>
                  ))}
                </div>
                <div className="mt-2 text-xs text-fg-muted">{d.inference.last_backend_evidence ? t.status.inference.evidence(d.inference.last_backend_evidence) : t.status.inference.fingerprint}</div>
              </div>
            </>
          ) : (
            <div className="text-sm">
              <p className="text-fg-muted">{t.status.inference.none}</p>
              <SetupHint title={dd.inference.howTitle} text={dd.inference.how(d.store.path)} />
            </div>
          )}
        </Card>
      </div>

      {d.caveats.length ? (
        <details className="card px-5 py-3 text-sm group" role="note">
          <summary className="flex items-center gap-2 cursor-pointer list-none [&::-webkit-details-marker]:hidden">
            <Icon name="info" size={18} className="text-fg-muted" />
            <span className="font-medium">{t.status.caveats}</span>
            <span className="text-fg-muted tnum">({d.caveats.length})</span>
            <Icon name="chevron" size={16} className="ml-auto text-fg-muted transition-transform group-open:rotate-180" />
          </summary>
          <ul className="mt-3 divide-y divide-line text-fg-muted max-w-[60rem]">
            {d.caveats.map((c, i) => (
              <li key={i} className="py-2 leading-relaxed wrap-soft">
                {c}
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      <footer className="flex flex-wrap items-center gap-x-3 gap-y-1 border-t border-line pt-4 text-xs text-fg-muted">
        <span className="inline-flex items-center gap-1.5 font-medium text-fg">
          <Dot tone={overall === "danger" ? "danger" : overall === "warn" ? "warn" : overall === "ok" ? "ok" : "off"} />
          {dd.footerReady}
        </span>
        <span>
          {t.status.footer.ledgerMoved} <a className="link" href={href("usage")}>{t.nav.usage}</a>
        </span>
        <span>
          {t.status.footer.profile} <code className="text-fg">{d.policy.profile}</code>
        </span>
        <span>
          {t.status.footer.piiScan} {d.policy.pii_scan ? t.common.on : t.common.off}
        </span>
        <span>{t.status.footer.auditRows(num(d.policy.audit_rows))}</span>
        <a className="link" href={href("compliance")}>
          {t.status.footer.compliance}
        </a>
        <span className="sm:ml-auto font-mono">cyberbrain {d.version}</span>
      </footer>
    </div>
  );
}

/** "How do I set this up", as a button that opens the answer instead of a wall of text. */
function SetupHint({ title, text }: { title: string; text: string }) {
  return (
    <details className="mt-3 group">
      <summary className="btn btn-win list-none inline-flex [&::-webkit-details-marker]:hidden">
        <Icon name="info" size={18} />
        {title}
        <Icon name="chevron" size={16} className="transition-transform group-open:rotate-180" />
      </summary>
      <p className="mt-2 rounded-lg bg-surface-2 border border-line px-3 py-2.5 text-sm text-fg-muted wrap-soft">{text}</p>
    </details>
  );
}

/** Always open when there is nothing to hide; a plain disclosure otherwise. */
function Details({ open, title, children }: { open: boolean; title: string | null; children: ReactNode }) {
  if (open || !title) return <>{children}</>;
  return (
    <details className="group">
      <summary className="inline-flex items-center gap-1.5 cursor-pointer text-sm text-fg-muted list-none [&::-webkit-details-marker]:hidden hover:text-fg">
        <Icon name="chevron" size={16} className="-rotate-90 transition-transform group-open:rotate-0" />
        {title}
      </summary>
      <div className="mt-2">{children}</div>
    </details>
  );
}
