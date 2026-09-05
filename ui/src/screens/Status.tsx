import { useState } from "react";
import { api, type DoctorReport, type InferenceBackend, type Ring, type ScanReport } from "@/api/client";
import { RingGlyph } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Dot, ErrorBanner, KeyValue, Loading, Pill, Section, Stat } from "@/components/ui";
import { absTime, bytes, num, relTime, RING_LABEL } from "@/lib/format";
import { useShortcuts } from "@/lib/keys";
import { href } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

const BACKEND_LABEL: Record<InferenceBackend, string> = {
  ollama: "Ollama",
  "lm-studio": "LM Studio",
  "nvidia-pair": "NVIDIA PAIR",
  unknown: "unknown",
};

export function StatusScreen() {
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
      toast(`scan: ${r.changed} changed, ${r.blocks_written} blocks in ${r.elapsed_ms} ms`);
      s.reload();
    } catch (e) {
      toast(toApiError(e).message, "err");
    } finally {
      setBusy("");
    }
  };

  useShortcuts("status", [
    { keys: "r", label: "Refresh", run: () => s.reload() },
    { keys: "d", label: "Run doctor", run: runDoctor },
  ]);

  if (s.error) return <div className="p-5"><ErrorBanner error={s.error} onRetry={s.reload} /></div>;
  const d = s.data;
  if (!d) return <Loading label="reading status" />;

  const capPct = Math.round((d.store.resident_cap.used / d.store.resident_cap.tokens) * 100);
  const maxRingBlocks = Math.max(1, ...d.store.rings.map((r) => r.blocks));
  const indexFresh = d.index.stale_notes === 0 && d.index.orphan_vectors === 0 && d.embedding.matches_index && d.index.fts_ok;
  const endpointTone = d.inference.endpoint_class === "public" ? (d.inference.allow_public_endpoint ? "warn" : "danger") : "ok";

  return (
    <div className="p-5 space-y-4 overflow-auto scroll-thin h-full">
      <div className="flex items-center gap-3 flex-wrap">
        <h1 className="text-lg font-semibold">Status</h1>
        <span className="font-mono text-xs text-fg-muted">cyberbrain {d.version}</span>
        <div className="ml-auto flex gap-1.5">
          <button className="btn btn-sm" onClick={() => s.reload()} disabled={s.loading}>
            refresh
          </button>
          <button className="btn btn-sm" onClick={runDoctor} disabled={busy !== ""}>
            {busy === "doctor" ? "checking…" : "doctor"}
          </button>
          <button className="btn btn-sm" onClick={() => runScan(false)} disabled={busy !== ""}>
            {busy === "scan" ? "scanning…" : "scan"}
          </button>
          <button className="btn btn-sm" onClick={() => runScan(true)} disabled={busy !== ""} title="Rebuild blocks, vectors and links for every note">
            {busy === "full" ? "rebuilding…" : "scan --full"}
          </button>
        </div>
      </div>

      <div className="grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="store" value={bytes(d.store.bytes)} sub={`${num(d.store.notes)} notes · ${num(d.store.blocks)} blocks · ${num(d.store.vectors)} vectors`} />
        <Stat label="index" value={indexFresh ? "fresh" : `${d.index.stale_notes} stale`} sub={`last scan ${relTime(d.index.last_scan)} · full ${relTime(d.index.last_full_scan)}`} tone={indexFresh ? "ok" : "warn"} />
        <Stat label="embedding" value={`${d.embedding.model.split("/").pop()} · d${d.embedding.dim}`} sub={d.embedding.matches_index ? "profile matches the index" : "PROFILE MISMATCH — semantic search off"} tone={d.embedding.matches_index ? undefined : "danger"} mono />
        <Stat label="last inference backend" value={BACKEND_LABEL[d.inference.last_backend]} sub={d.inference.last_call ? `answered ${relTime(d.inference.last_call)}` : "no call yet"} tone={d.inference.last_backend === "unknown" ? "warn" : undefined} />
      </div>

      {!d.embedding.matches_index ? (
        <div className="panel border-danger/60 bg-danger-bg px-4 py-3 text-sm" role="alert">
          <span className="font-medium text-danger">Semantic search is disabled.</span> The configured embedding profile <code>{d.embedding.profile_id}</code> does not match the vectors in the index. Recall runs lexical-only and says so. Run <code>scan --full</code> to reindex; comparing vectors across models would degrade results silently, which is why it is not allowed.
        </div>
      ) : null}

      <div className="grid gap-4 lg:grid-cols-2">
        <Section title="Store" aside={<code className="text-2xs">{d.store.path}</code>}>
          <KeyValue
            rows={[
              ["notes/", `${bytes(d.store.notes_bytes)} · Markdown only, authoritative`],
              ["cyberbrain.db", `${bytes(d.store.db_bytes)} · index, vectors, audit; rebuildable`],
              ["models/", `${bytes(d.store.models_bytes)} · content-addressed artefacts`],
            ]}
          />
          <div className="mt-4">
            <div className="flex items-baseline justify-between">
              <span className="label">resident cap · rings 0+1</span>
              <span className="text-xs tnum text-fg-muted">
                {num(d.store.resident_cap.used)} / {num(d.store.resident_cap.tokens)} tokens · {capPct}%
              </span>
            </div>
            <div className="mt-1 h-2 rounded-sm bg-surface-3 overflow-hidden">
              <div className="h-full" style={{ width: `${Math.min(100, capPct)}%`, background: capPct > 90 ? "var(--danger)" : capPct > 70 ? "var(--warn)" : "var(--ok)" }} />
            </div>
            <div className="mt-1 text-2xs text-fg-faint">Injected into every session. Exceeding the cap is a write-time error, never a read-time truncation.</div>
          </div>
          <div className="mt-4">
            <div className="label mb-1.5">blocks per ring</div>
            <ul className="space-y-1">
              {d.store.rings.map((r) => (
                <li key={r.ring} className="grid grid-cols-[5.5rem_1fr_9rem] items-center gap-2 text-xs">
                  <a href={href("notes", null, { ring: r.ring })} className="inline-flex items-center gap-1.5 hover:underline">
                    <RingGlyph ring={r.ring as Ring} size={9} />
                    <span className="font-mono" style={{ color: `var(--ring-${r.ring})` }}>
                      r{r.ring}
                    </span>
                    <span className="text-fg-muted">{RING_LABEL[r.ring]}</span>
                  </a>
                  <div className="h-1.5 rounded-sm bg-surface-3 overflow-hidden">
                    <div className="h-full" style={{ width: `${(r.blocks / maxRingBlocks) * 100}%`, background: `var(--ring-${r.ring})` }} />
                  </div>
                  <span className="tnum text-fg-muted text-right">
                    {num(r.notes)} n · {num(r.blocks)} b · {num(r.tokens)} t
                  </span>
                </li>
              ))}
            </ul>
          </div>
        </Section>

        <Section title="Index" aside={<span>schema v{d.index.schema_version}</span>}>
          <KeyValue
            rows={[
              ["last scan", <span title={absTime(d.index.last_scan)}>{relTime(d.index.last_scan)}</span>],
              ["last full scan", <span title={absTime(d.index.last_full_scan)}>{relTime(d.index.last_full_scan)}</span>],
              ["stale notes", <span className={d.index.stale_notes ? "text-warn" : ""}>{d.index.stale_notes} <span className="text-fg-faint">(mtime or hash differs from the index)</span></span>],
              ["orphan vectors", <span className={d.index.orphan_vectors ? "text-danger" : "text-ok"}>{d.index.orphan_vectors}</span>],
              ["dangling links", <span>{d.index.dangling_links} <span className="text-fg-faint">(intent, listed by doctor)</span></span>],
              ["FTS5", d.index.fts_ok ? <span className="text-ok">ok</span> : <span className="text-danger">broken</span>],
            ]}
          />
          {scan ? (
            <div className="mt-3 panel px-3 py-2 text-xs">
              <Pill>{scan.full ? "scan --full" : "scan"}</Pill> <span className="tnum">{scan.scanned} scanned, {scan.changed} changed, {scan.added} added, {scan.removed} removed, {scan.blocks_written} blocks and {scan.vectors_written} vectors written in {scan.elapsed_ms} ms</span>
            </div>
          ) : null}
          {doctor ? (
            <div className="mt-3">
              <div className="flex items-center gap-2 text-xs">
                <Dot tone={doctor.ok ? "ok" : "danger"} />
                <span className="font-medium">doctor {doctor.ok ? "clean" : "found errors"}</span>
                <span className="text-fg-faint tnum">{doctor.findings.length} findings · {relTime(doctor.checked_at)}</span>
              </div>
              <ul className="mt-1.5 space-y-0.5 text-xs max-h-56 overflow-auto scroll-thin">
                {doctor.findings.map((f, i) => (
                  <li key={i} className="flex gap-2">
                    <Pill tone={f.severity === "error" ? "danger" : f.severity === "warn" ? "warn" : "neutral"}>{f.check}</Pill>
                    <span className="font-mono text-fg-muted truncate max-w-[12rem]">{f.subject}</span>
                    <span className="text-fg-muted">{f.message}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </Section>

        <Section title="Embedding profile" aside={<Pill tone={d.embedding.matches_index ? "ok" : "danger"}>{d.embedding.matches_index ? "matches index" : "mismatch"}</Pill>}>
          <KeyValue
            rows={[
              ["profile id", <code className="text-xs">{d.embedding.profile_id}</code>],
              ["model", <code className="text-xs">{d.embedding.model}</code>],
              ["dimension", `${d.embedding.dim} · ${d.embedding.pooling}`],
              ["backend", `${d.embedding.backend} (${d.embedding.backend === "static" ? "token lookup, no transformer at query time" : "candle transformer"})`],
              ["artefact hash", <code className="text-xs break-all">{d.embedding.model_hash}</code>],
              ["verified", <span title={absTime(d.embedding.model_verified_at)}>{relTime(d.embedding.model_verified_at)} on load · mismatch is a hard failure</span>],
            ]}
          />
        </Section>

        <Section title="Inference endpoint" aside={d.inference.configured ? <Pill tone={endpointTone}>{d.inference.endpoint_class}</Pill> : <Pill>not configured</Pill>}>
          {d.inference.configured ? (
            <>
              <KeyValue
                rows={[
                  ["base url", <code className="text-xs">{d.inference.base_url}</code>],
                  ["address class", <span className={endpointTone === "ok" ? "" : endpointTone === "warn" ? "text-warn" : "text-danger"}>{d.inference.endpoint_class}{d.inference.endpoint_class === "public" ? (d.inference.allow_public_endpoint ? " · allowed by allow_public_endpoint = true" : " · REFUSED: allow_public_endpoint is false") : " · stays on this machine or your network"}</span>],
                  ["model", d.inference.model ?? "—"],
                  ["reachable", d.inference.reachable === null ? "not probed" : <span className={d.inference.reachable ? "text-ok" : "text-warn"}>{d.inference.reachable ? "yes" : "no"} <span className="text-fg-faint">({relTime(d.inference.reachable_checked_at)}, probed by the binary, never by this page)</span></span>],
                  ["last call", <span title={absTime(d.inference.last_call)}>{relTime(d.inference.last_call)}</span>],
                ]}
              />
              <div className="mt-4 panel px-3 py-2.5">
                <div className="label">which backend answered last</div>
                <div className="mt-1 flex items-center gap-2">
                  {(["ollama", "lm-studio", "nvidia-pair", "unknown"] as InferenceBackend[]).map((b) => (
                    <span key={b} className={`inline-flex items-center h-6 px-2 rounded border text-xs ${d.inference.last_backend === b ? "btn-primary font-medium" : "text-fg-faint border-line"}`}>
                      {BACKEND_LABEL[b]}
                    </span>
                  ))}
                </div>
                <div className="mt-1.5 text-2xs text-fg-faint">{d.inference.last_backend_evidence ? `evidence: ${d.inference.last_backend_evidence}` : "identified from the response fingerprint; unknown means the server matched no known signature, which is not an error"}</div>
              </div>
            </>
          ) : (
            <p className="text-sm text-fg-muted">No inference endpoint configured. Every core function works without one; contradiction checks, ring proposals and session summaries are off, and recall says so in its caveats.</p>
          )}
        </Section>
      </div>

      <div className="text-2xs text-fg-faint">
        policy profile <code className="text-fg">{d.policy.profile}</code> · PII scan {d.policy.pii_scan ? "on" : "off"} · {num(d.policy.audit_rows)} audit rows · <a className="link" href={href("compliance")}>compliance</a>
      </div>
    </div>
  );
}
