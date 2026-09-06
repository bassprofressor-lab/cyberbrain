import { useState } from "react";
import { api, type DoctorFinding, type DoctorReport, type InferenceBackend, type Ring, type ScanReport } from "@/api/client";
import { RingGlyph } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Dot, ErrorBanner, KeyValue, Loading, Pill, Section, Stat } from "@/components/ui";
import { absTime, bytes, num, relTime } from "@/lib/format";
import { type Dict, useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { href } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

const BACKEND_LABEL: Record<InferenceBackend, string> = {
  ollama: "Ollama",
  "lm-studio": "LM Studio",
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
  if (f.check === "unresolvable-links") return { label: t.status.doctor.unresolvable, tone: "warn", hint: t.status.doctor.unresolvableHint };
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

  const capPct = Math.round((d.store.resident_cap.used / d.store.resident_cap.tokens) * 100);
  const maxRingBlocks = Math.max(1, ...d.store.rings.map((r) => r.blocks));
  const indexFresh = d.index.stale_notes === 0 && d.index.orphan_vectors === 0 && d.embedding.matches_index !== false && d.index.fts_ok;
  const endpointTone = d.inference.endpoint_class === "public" ? (d.inference.allow_public_endpoint ? "warn" : "danger") : "ok";
  const backendLabel = (b: InferenceBackend) => (b === "unknown" ? t.status.stat.backendUnknown : BACKEND_LABEL[b]);

  return (
    <div className="p-6 space-y-6 overflow-auto scroll-thin h-full">
      <div className="flex items-center gap-3 flex-wrap">
        <h1 className="text-lg font-semibold">{t.status.title}</h1>
        <span className="font-mono text-xs text-fg-muted">cyberbrain {d.version}</span>
        <div className="ml-auto flex gap-1.5">
          <button className="btn btn-sm" onClick={() => s.reload()} disabled={s.loading}>
            {t.common.refresh}
          </button>
          <button className="btn btn-sm" onClick={runDoctor} disabled={busy !== ""}>
            {busy === "doctor" ? t.status.btn.doctorBusy : t.status.btn.doctor}
          </button>
          <button className="btn btn-sm" onClick={() => runScan(false)} disabled={busy !== ""}>
            {busy === "scan" ? t.status.btn.scanBusy : t.status.btn.scan}
          </button>
          <button className="btn btn-sm" onClick={() => runScan(true)} disabled={busy !== ""} title={t.status.btn.fullTitle}>
            {busy === "full" ? t.status.btn.fullBusy : t.status.btn.full}
          </button>
        </div>
      </div>

      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
        <Stat label={t.status.stat.store} value={bytes(d.store.bytes)} sub={t.status.stat.storeSub(num(d.store.notes), num(d.store.blocks), num(d.store.vectors))} />
        <Stat label={t.status.stat.index} value={indexFresh ? t.status.stat.indexFresh : t.status.stat.indexStale(d.index.stale_notes)} sub={t.status.stat.indexSub(relTime(d.index.last_scan), relTime(d.index.last_full_scan))} tone={indexFresh ? "ok" : "warn"} />
        <Stat
          label={t.status.stat.embedding}
          value={d.embedding.loaded ? `${d.embedding.model.split("/").pop()} · d${d.embedding.dim}` : t.status.stat.embeddingNone}
          sub={!d.embedding.loaded ? t.status.stat.embeddingLexical : d.embedding.matches_index === null ? t.status.stat.embeddingNothing : d.embedding.matches_index ? t.status.stat.embeddingMatches : t.status.stat.embeddingMismatch}
          tone={!d.embedding.loaded ? "warn" : d.embedding.matches_index === false ? "danger" : undefined}
          mono
        />
        <Stat label={t.status.stat.backend} value={backendLabel(d.inference.last_backend)} sub={d.inference.last_call ? t.status.stat.backendAnswered(relTime(d.inference.last_call)) : t.status.stat.backendNoCall} tone={d.inference.last_backend === "unknown" ? "warn" : undefined} />
      </div>

      {d.caveats.length ? (
        <div className="panel px-4 py-2.5 text-xs" role="note">
          <div className="label">{t.status.caveats}</div>
          <ul className="mt-1 space-y-0.5 text-fg-muted">
            {d.caveats.map((c, i) => (
              <li key={i}>· {c}</li>
            ))}
          </ul>
        </div>
      ) : null}

      {d.embedding.matches_index === false ? (
        <div className="panel border-danger/60 bg-danger-bg px-4 py-3 text-sm" role="alert">
          <span className="font-medium text-danger">{t.status.mismatch.lead}</span> {t.status.mismatch.body(d.embedding.profile_id)}
        </div>
      ) : null}

      <div className="grid gap-5 xl:grid-cols-2">
        <Section title={t.status.store.title} aside={<code className="text-2xs">{d.store.path}</code>}>
          <KeyValue
            rows={[
              [t.status.store.notes, t.status.store.notesValue(bytes(d.store.notes_bytes))],
              [t.status.store.db, t.status.store.dbValue(bytes(d.store.db_bytes))],
              [t.status.store.models, t.status.store.modelsValue(bytes(d.store.models_bytes))],
            ]}
          />
          <div className="mt-4">
            <div className="flex items-baseline justify-between">
              <span className="label">{t.status.store.cap}</span>
              <span className="text-xs tnum text-fg-muted">{t.status.store.capValue(num(d.store.resident_cap.used), num(d.store.resident_cap.tokens), capPct)}</span>
            </div>
            <div className="mt-1 h-2 rounded-sm bg-surface-3 overflow-hidden">
              <div className="h-full" style={{ width: `${Math.min(100, capPct)}%`, background: capPct > 90 ? "var(--danger)" : capPct > 70 ? "var(--warn)" : "var(--ok)" }} />
            </div>
            <div className="mt-1 text-2xs text-fg-faint">{t.status.store.capNote}</div>
          </div>
          <div className="mt-4">
            <div className="label mb-1.5">{t.status.store.blocksPerRing}</div>
            <ul className="space-y-1">
              {d.store.rings.map((r) => (
                <li key={r.ring} className="grid grid-cols-[5.5rem_1fr_10.5rem] items-center gap-2 text-xs">
                  <a href={href("notes", null, { ring: r.ring })} className="inline-flex items-center gap-1.5 hover:underline">
                    <RingGlyph ring={r.ring as Ring} size={9} />
                    <span className="font-mono" style={{ color: `var(--ring-${r.ring})` }}>
                      r{r.ring}
                    </span>
                    <span className="text-fg-muted">{t.rings.label[r.ring]}</span>
                  </a>
                  <div className="h-1.5 rounded-sm bg-surface-3 overflow-hidden">
                    <div className="h-full" style={{ width: `${(r.blocks / maxRingBlocks) * 100}%`, background: `var(--ring-${r.ring})` }} />
                  </div>
                  <span className="tnum text-fg-muted text-right whitespace-nowrap">{t.status.store.ringCounts(num(r.notes), num(r.blocks), num(r.tokens))}</span>
                </li>
              ))}
            </ul>
          </div>
        </Section>

        <Section title={t.status.index.title} aside={<span>{t.status.index.schema(d.index.schema_version)}</span>}>
          <KeyValue
            rows={[
              [t.status.index.lastScan, <span title={absTime(d.index.last_scan)}>{relTime(d.index.last_scan)}</span>],
              [t.status.index.lastFull, <span title={absTime(d.index.last_full_scan)}>{relTime(d.index.last_full_scan)}</span>],
              [t.status.index.stale, <span className={d.index.stale_notes ? "text-warn" : ""}>{d.index.stale_notes} <span className="text-fg-faint">{t.status.index.staleHint}</span></span>],
              [t.status.index.orphans, <span className={d.index.orphan_vectors ? "text-danger" : "text-ok"}>{d.index.orphan_vectors}</span>],
              [t.status.index.dangling, <span>{d.index.dangling_links} <span className="text-fg-faint">{t.status.index.danglingHint}</span></span>],
              [t.status.index.fts, d.index.fts_ok ? <span className="text-ok">{t.status.index.ftsOk}</span> : <span className="text-danger">{t.status.index.ftsBroken}</span>],
            ]}
          />
          {scan ? (
            <div className="mt-3 panel px-3 py-2 text-xs">
              <Pill>{scan.full ? t.status.btn.full : t.status.btn.scan}</Pill>{scan.dry_run ? <Pill className="ml-1">{t.status.index.dryRun}</Pill> : null}{" "}
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
            <div className="mt-3">
              <div className="flex items-center gap-2 text-xs flex-wrap">
                <Dot tone={doctor.ok ? "ok" : doctor.findings.some((f) => f.severity === "error") ? "danger" : "warn"} />
                <span className="font-medium">doctor {doctor.ok ? t.status.doctor.clean : doctor.findings.some((f) => f.severity === "error") ? t.status.doctor.errors : t.status.doctor.warnings}</span>
                <span className="text-fg-faint tnum">{t.status.doctor.summary(doctor.findings.length, doctor.checks_run.length, relTime(doctor.checked_at))}</span>
              </div>
              <div className="mt-1 text-2xs text-fg-faint" title={doctor.checks_run.join(", ")}>
                {t.status.doctor.checks(doctor.checks_run.join(" · "))}
              </div>
              <ul className="mt-1.5 space-y-0.5 text-xs max-h-56 overflow-auto scroll-thin">
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
        </Section>

        <Section
          title={t.status.embedding.title}
          aside={!d.embedding.loaded ? <Pill tone="warn">{t.status.embedding.noModel}</Pill> : d.embedding.matches_index === null ? <Pill title={t.status.embedding.nothingToCompareTitle}>{t.status.embedding.nothingToCompare}</Pill> : <Pill tone={d.embedding.matches_index ? "ok" : "danger"}>{d.embedding.matches_index ? t.status.embedding.matches : t.status.embedding.mismatch}</Pill>}
        >
          {!d.embedding.loaded ? <p className="mb-3 text-xs text-warn">{t.status.embedding.warning}</p> : null}
          <KeyValue
            rows={[
              [t.status.embedding.profileId, <code className="text-xs">{d.embedding.profile_id}</code>],
              [t.status.embedding.model, <code className="text-xs">{d.embedding.model}</code>],
              [t.status.embedding.dimension, `${d.embedding.dim} · ${d.embedding.pooling}`],
              [t.status.embedding.backend, `${d.embedding.backend} (${d.embedding.backend === "static" ? t.status.embedding.backendStatic : t.status.embedding.backendCandle})`],
              [t.status.embedding.hash, d.embedding.model_hash ? <code className="text-xs break-all">{d.embedding.model_hash}</code> : <span className="text-fg-faint">{t.status.embedding.hashNone}</span>],
              [t.status.embedding.verified, d.embedding.model_verified_at ? <span title={absTime(d.embedding.model_verified_at)}>{t.status.embedding.verifiedAt(relTime(d.embedding.model_verified_at))}</span> : <span className="text-fg-muted">{t.status.embedding.verifiedNoStamp}</span>],
            ]}
          />
        </Section>

        <Section title={t.status.inference.title} aside={d.inference.configured ? <Pill tone={endpointTone}>{d.inference.endpoint_class}</Pill> : <Pill>{t.status.inference.notConfigured}</Pill>}>
          {d.inference.configured ? (
            <>
              <KeyValue
                rows={[
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
                      <span className={d.inference.reachable ? "text-ok" : "text-warn"}>
                        {d.inference.reachable ? t.common.yes : t.common.no} <span className="text-fg-faint">{t.status.inference.probedBy(relTime(d.inference.reachable_checked_at))}</span>
                      </span>
                    ),
                  ],
                  [t.status.inference.lastCall, d.inference.last_call ? <span title={absTime(d.inference.last_call)}>{relTime(d.inference.last_call)}</span> : <span className="text-fg-muted">{t.status.inference.noAuditRow}</span>],
                ]}
              />
              <div className="mt-4 panel px-3 py-2.5">
                <div className="label">{t.status.inference.whichBackend}</div>
                <div className="mt-1 flex items-center gap-2">
                  {(["ollama", "lm-studio", "nvidia-pair", "unknown"] as InferenceBackend[]).map((b) => (
                    <span key={b} className={`inline-flex items-center h-6 px-2 rounded border text-xs ${d.inference.last_backend === b ? "btn-primary font-medium" : "text-fg-faint border-line"}`}>
                      {backendLabel(b)}
                    </span>
                  ))}
                </div>
                <div className="mt-1.5 text-2xs text-fg-faint">{d.inference.last_backend_evidence ? t.status.inference.evidence(d.inference.last_backend_evidence) : t.status.inference.fingerprint}</div>
              </div>
            </>
          ) : (
            <p className="text-sm text-fg-muted">{t.status.inference.none}</p>
          )}
        </Section>
      </div>

      <div className="text-2xs text-fg-faint">
        {t.status.footer.ledgerMoved} <a className="link" href={href("usage")}>{t.nav.usage}</a> · {t.status.footer.profile} <code className="text-fg">{d.policy.profile}</code> · {t.status.footer.piiScan} {d.policy.pii_scan ? t.common.on : t.common.off} · {t.status.footer.auditRows(num(d.policy.audit_rows))} · <a className="link" href={href("compliance")}>{t.status.footer.compliance}</a>
      </div>
    </div>
  );
}
