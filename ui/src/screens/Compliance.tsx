import { useEffect, useMemo, useRef, useState } from "react";
import { api, AUDIT_ACTION_FAMILIES, type AuditAction, type AuditActionFilter, type AuditRow, type EgressPath, type PiiState, type RetentionApplyReport, type SubjectAccessReport } from "@/api/client";
import { CitationChip } from "@/components/Citation";
import { RingBadge } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Dot, Empty, ErrorBanner, KeyValue, Loading, Pill, Section, Stat } from "@/components/ui";
import { absTime, bytes, duration, num, relTime, shortDate } from "@/lib/format";
import { useShortcuts } from "@/lib/keys";
import { href, type Route } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

const SECTIONS = [
  ["overview", "Overview"],
  ["egress", "Egress register"],
  ["audit", "Audit log"],
  ["pii", "PII"],
  ["retention", "Retention"],
  ["models", "Model card"],
  ["subject", "Subject access"],
] as const;

/** The log's own vocabulary, by family: `note` selects `note.*`, and so on (`AuditActionFilter`). */
const ACTION_FILTERS = AUDIT_ACTION_FAMILIES;

function piiTone(state: PiiState): "ok" | "warn" | "danger" | "neutral" {
  return state === "flagged" ? "danger" : state === "reviewed" ? "warn" : state === "none" ? "ok" : "neutral";
}
const PII_LABEL: Record<PiiState, string> = { unscanned: "not scanned", none: "scanned, nothing found", reviewed: "reviewed", flagged: "flagged" };

export function ComplianceScreen({ route }: { route: Route }) {
  const egress = useAsync(() => api.egress(), []);
  const pii = useAsync(() => api.pii(), []);
  const retention = useAsync(() => api.retention(), []);
  const models = useAsync(() => api.modelCards(), []);
  const status = useAsync(() => api.status(), []);
  const scrollRef = useRef<HTMLDivElement>(null);
  const [active, setActive] = useState<string>(route.anchor ?? "overview");

  useEffect(() => {
    if (route.anchor) document.getElementById(`c-${route.anchor}`)?.scrollIntoView({ block: "start" });
  }, [route.anchor, egress.data]);

  useEffect(() => {
    const root = scrollRef.current;
    if (!root) return;
    const obs = new IntersectionObserver(
      (entries) => {
        const vis = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
        if (vis) setActive(vis.target.id.replace(/^c-/, ""));
      },
      { root, rootMargin: "-10% 0px -70% 0px" },
    );
    for (const [id] of SECTIONS) {
      const el = document.getElementById(`c-${id}`);
      if (el) obs.observe(el);
    }
    return () => obs.disconnect();
  }, [egress.data, pii.data, retention.data, models.data]);

  useShortcuts(
    "compliance",
    SECTIONS.map(([id, label], i) => ({ keys: String(i + 1), label: `Jump to ${label}`, run: () => document.getElementById(`c-${id}`)?.scrollIntoView({ block: "start" }) })),
    [],
  );

  const profile = egress.data?.profile ?? status.data?.policy.profile;

  return (
    <div className="grid h-full grid-cols-1 md:grid-cols-[12rem_1fr]">
      <nav className="border-r hidden md:block py-3">
        <ul className="text-sm">
          {SECTIONS.map(([id, label], i) => (
            <li key={id}>
              <a href={href("compliance", null, undefined, id)} className={`flex items-center gap-2 px-4 py-1.5 ${active === id ? "row-selected font-medium" : "text-fg-muted hover:text-fg"}`}>
                <span className="font-mono text-2xs text-fg-faint w-3">{i + 1}</span>
                {label}
              </a>
            </li>
          ))}
        </ul>
        {profile ? (
          <div className="px-4 mt-4 text-2xs text-fg-faint">
            profile <code className={profile === "off" ? "text-warn" : "text-fg"}>{profile}</code>
            <div className="mt-1 leading-relaxed">{profile === "eu" ? "GDPR, EU AI Act" : profile === "ch" ? "revised FADP" : "checks compiled in, disabled"}</div>
          </div>
        ) : null}
      </nav>
      <div ref={scrollRef} className="overflow-auto scroll-thin p-5 space-y-5 min-w-0">
        <div id="c-overview">
          {egress.error ? <ErrorBanner error={egress.error} onRetry={egress.reload} /> : egress.data ? <Overview register={egress.data} auditRows={status.data?.policy.audit_rows ?? null} /> : <Loading label="reading egress register" />}
        </div>

        <Section id="c-egress" title="Egress register" aside={egress.data ? <span className="font-mono">register {egress.data.register_hash}</span> : null}>
          {egress.data ? <EgressTable paths={egress.data.paths} /> : null}
          <p className="mt-3 text-xs text-fg-muted leading-relaxed">
            Every code path that can send bytes off the machine is registered at compile time in one module; all outbound I/O goes through a single wrapper that takes one of these purposes, and CI fails on any HTTP client built elsewhere. A purpose not on this list does not exist in the binary. Telemetry is not on the list.
          </p>
        </Section>

        <div id="c-audit">
          <AuditSection />
        </div>

        <Section id="c-pii" title="PII findings" aside={pii.data ? <Pill tone={pii.data.scan_enabled ? "ok" : "warn"}>{pii.data.scan_enabled ? "write-time scan on" : "scan off (profile off)"}</Pill> : null}>
          {pii.error ? <ErrorBanner error={pii.error} onRetry={pii.reload} /> : null}
          {pii.data ? (
            <>
              {pii.data.holds.length ? (
                <div className="mb-3 panel border-warn/50 px-3 py-2 text-sm">
                  <span className="text-warn font-medium">{pii.data.holds.length} write{pii.data.holds.length === 1 ? "" : "s"} held</span> awaiting a decision:{" "}
                  {pii.data.holds.map((h) => (
                    <a key={h.hold_id} href={href("note", h.note)} className="link mr-2">
                      {h.note}
                    </a>
                  ))}
                </div>
              ) : null}
              {pii.data.entries.length ? (
                <table className="w-full text-xs">
                  <thead>
                    <tr className="label text-left">
                      <th className="py-1 pr-3 font-medium">note</th>
                      <th className="py-1 pr-3 font-medium">state</th>
                      <th className="py-1 pr-3 font-medium">findings (masked)</th>
                      <th className="py-1 font-medium">reviewed</th>
                    </tr>
                  </thead>
                  <tbody>
                    {pii.data.entries.map((e) => (
                      <tr key={e.note.id} className="border-t align-top">
                        <td className="py-1.5 pr-3">
                          <span className="inline-flex items-center gap-1.5">
                            <RingBadge ring={e.note.ring} />
                            <a href={href("note", e.note.name)} className="link font-mono">
                              {e.note.name}
                            </a>
                          </span>
                        </td>
                        <td className="py-1.5 pr-3">
                          <Pill tone={piiTone(e.state)} title={e.state === "unscanned" ? "No write-time scan ever ran over this note (imported or hand-written). The findings column is a scan of the body as it is now." : undefined}>
                            {PII_LABEL[e.state]}
                          </Pill>
                        </td>
                        <td className="py-1.5 pr-3 font-mono">
                          {e.findings.length ? (
                            e.findings.map((f, i) => (
                              <div key={i}>
                                <span className="text-fg-muted">{f.kind}</span> {f.excerpt} <span className="text-fg-faint tnum">{f.line}:{f.col}</span>
                              </div>
                            ))
                          ) : (
                            <span className="text-fg-faint font-sans">{e.state === "unscanned" ? "a scan of the body now finds nothing; the note stays unscanned until it is written through the tool" : "nothing found in the body now"}</span>
                          )}
                        </td>
                        <td className="py-1.5 text-fg-muted" title={absTime(e.reviewed_at)}>
                          {e.reviewed_at ? relTime(e.reviewed_at) : <span className="text-fg-faint">never scanned</span>}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <Empty title="Every note was scanned and none carries reviewed or flagged personal data.">Every write under profile eu/ch is scanned for e-mail addresses, IPs, API keys, IBANs and phone numbers before it lands. Heuristic: a seatbelt, not a guarantee. A note nobody scanned would be listed here as “not scanned”.</Empty>
              )}
            </>
          ) : null}
        </Section>

        <div id="c-retention">
          <RetentionSection />
        </div>

        <Section id="c-models" title="Model card" aside={<span>EU AI Act transparency · what generates or embeds on this machine</span>}>
          {models.error ? <ErrorBanner error={models.error} onRetry={models.reload} /> : null}
          {models.data ? (
            <div className="grid gap-3 lg:grid-cols-2">
              {models.data.map((m) => (
                <div key={m.role + m.name} className="panel p-3 text-sm">
                  <div className="flex items-center gap-2">
                    <Pill tone={m.active ? "ok" : "neutral"}>{m.role}</Pill>
                    <span className="font-mono font-medium">{m.name}</span>
                    {m.active ? <span className="ml-auto text-2xs text-fg-faint">in use</span> : null}
                  </div>
                  <div className="mt-2">
                    <KeyValue
                      rows={[
                        ["source", <span className="text-xs">{m.source}</span>],
                        ["licence", m.license === "not stated" ? <span className="text-fg-muted">not stated by the source; never guessed</span> : m.license],
                        ["hash", m.hash ? <code className="text-xs break-all">{m.hash}</code> : <span className="text-fg-faint">not held (endpoint model; weights are not on this machine)</span>],
                        ["format", m.format ?? <span className="text-fg-faint">not stated</span>],
                        ["dimension", m.dim ? `${m.dim} · ${m.pooling ?? "pooling not stated"}` : <span className="text-fg-faint">{m.role === "inference" ? "not applicable (generates text)" : "not stated"}</span>],
                        ["size", m.bytes === null ? <span className="text-fg-faint">{m.role === "inference" ? "not held" : "not stated"}</span> : bytes(m.bytes)],
                        ["verified", m.verified_at ? <span title={absTime(m.verified_at)}>{relTime(m.verified_at)} on load</span> : m.hash ? <span className="text-fg-muted">hash checked on every load; no timestamp of that check is kept</span> : <span className="text-fg-faint">nothing to verify</span>],
                      ]}
                    />
                  </div>
                  <div className="mt-2 text-xs">
                    <div className="label">intended use</div>
                    <p className="mt-0.5 text-fg-muted leading-relaxed">{m.intended_use}</p>
                    <div className="label mt-2">limitations</div>
                    <p className="mt-0.5 text-fg-muted leading-relaxed">{m.limitations}</p>
                  </div>
                </div>
              ))}
            </div>
          ) : null}
        </Section>

        <div id="c-subject">
          <SubjectSection />
        </div>
      </div>
    </div>
  );
}

function Overview({ register, auditRows }: { register: Awaited<ReturnType<typeof api.egress>>; auditRows: number | null }) {
  const pub = register.paths.filter((p) => p.destination_class === "public");
  const local = register.paths.filter((p) => p.destination_class !== "public");
  const pubUses = pub.reduce((a, p) => a + p.uses_total, 0);
  const pubBytes = pub.reduce((a, p) => a + p.bytes_out_total, 0);
  const localUses = local.reduce((a, p) => a + p.uses_total, 0);
  const pubEnabled = pub.filter((p) => p.enabled);
  const days = Math.max(1, Math.round((Date.now() - Date.parse(register.since)) / 86_400_000));
  const lastPub = pub.map((p) => p.last_used).filter(Boolean).sort().pop() ?? null;
  const anyLocalPublic = local.some((p) => p.destination_class === "unresolved");

  // The headline is computed from the register, not typed in. If the register says
  // something left, the headline says so.
  let tone: "ok" | "warn" = "ok";
  let headline: string;
  let sub: string;
  if (pubUses === 0) {
    headline = "Nothing has left this machine.";
    sub = `In ${days} days no registered path sent a byte to a public destination. ${pubEnabled.length ? `${pubEnabled.length} such path${pubEnabled.length === 1 ? " is" : "s are"} enabled and unused.` : "No such path is currently enabled."}`;
  } else if (pub.every((p) => p.purpose === "model-download")) {
    headline = "No note content has left this machine.";
    sub = `The only outbound traffic to the internet in ${days} days was ${pubUses} model download${pubUses === 1 ? "" : "s"} (${bytes(pubBytes)} sent, on your consent, ${relTime(lastPub)}). That request carried no note text, no identifier, no telemetry. ${pubEnabled.length === 0 ? "The download path is now disabled: the artefact is present and hash-verified." : ""}`;
  } else {
    tone = "warn";
    headline = "Bytes have left this machine.";
    sub = `${pubUses} outbound request${pubUses === 1 ? "" : "s"} to public destinations in ${days} days (${bytes(pubBytes)}). See the register below for which purposes.`;
  }
  if (anyLocalPublic) tone = "warn";

  return (
    <div className="panel p-5" style={{ borderColor: tone === "ok" ? "color-mix(in oklch, var(--ok) 45%, var(--line))" : "color-mix(in oklch, var(--warn) 55%, var(--line))" }}>
      <div className="flex items-start gap-3">
        <div className="mt-1.5">
          <Dot tone={tone} />
        </div>
        <div className="min-w-0">
          <h1 className="text-xl font-semibold leading-tight">{headline}</h1>
          <p className="mt-1.5 text-sm text-fg-muted leading-relaxed max-w-3xl">{sub}</p>
        </div>
        <div className="ml-auto text-right text-2xs text-fg-faint shrink-0">
          <div>profile <code className="text-fg">{register.profile}</code></div>
          <div title={absTime(register.since)}>since {shortDate(register.since)}</div>
        </div>
      </div>
      <div className="mt-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label="to the internet" value={pubUses === 0 ? "0 requests" : `${num(pubUses)} request${pubUses === 1 ? "" : "s"}`} sub={pubUses === 0 ? "no bytes to any public host" : `${bytes(pubBytes)} out · ${pub.map((p) => p.purpose).join(", ")}`} tone={pubUses === 0 ? "ok" : undefined} />
        <Stat label="to your own network" value={`${num(localUses)} call${localUses === 1 ? "" : "s"}`} sub={local.length ? `${local.map((p) => p.destination.replace(/^https?:\/\//, "")).join(", ")} · ${local.map((p) => p.destination_class).join(", ")}` : "no local endpoint configured"} />
        <Stat label="refused by the wrapper" value={num(register.refused_total)} sub={register.refused_total ? "attempts outside policy, each logged" : "no attempt outside policy"} tone={register.refused_total ? "warn" : "ok"} />
        <Stat label="audit rows" value={auditRows === null ? "—" : num(auditRows)} sub="append-only, exportable" />
      </div>
      <div className="mt-4 grid gap-x-6 gap-y-1.5 sm:grid-cols-2 text-xs text-fg-muted">
        <div className="label sm:col-span-2">how this statement is known, not hoped</div>
        <div>· The list of purposes is closed at compile time (register hash <code className="text-fg">{register.register_hash}</code>). A path not on it cannot be built.</div>
        <div>· All outbound I/O goes through one wrapper that requires a registered purpose; CI fails on an HTTP client constructed anywhere else.</div>
        <div>· Every use of a path writes an <code>egress.permitted</code> row and, when the request closes, an <code>egress.completed</code> row with the bytes. “Uses” count the former, “bytes out” sum the latter; a request that never closed counts as a use with no bytes.</div>
        <div>· The inference endpoint must be loopback or private-range unless <code>allow_public_endpoint</code> is set; a refusal is logged as <code>policy.refusal</code>.</div>
        <div>· The core embeds statically; no model server, no system library, no runtime fetch. The UI you are reading is served from the binary and fetches nothing external.</div>
        <div>· Telemetry is not a purpose. There is no opt-out because there is nothing to opt out of.</div>
      </div>
    </div>
  );
}

function EgressTable({ paths }: { paths: EgressPath[] }) {
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-xs min-w-[48rem]">
        <thead>
          <tr className="label text-left">
            <th className="py-1 pr-3 font-medium">purpose</th>
            <th className="py-1 pr-3 font-medium">state</th>
            <th className="py-1 pr-3 font-medium">destination</th>
            <th className="py-1 pr-3 font-medium">what is sent</th>
            <th className="py-1 pr-3 font-medium">permitted by</th>
            <th className="py-1 pr-3 font-medium tnum">uses</th>
            <th className="py-1 pr-3 font-medium tnum">bytes out</th>
            <th className="py-1 font-medium">last used</th>
          </tr>
        </thead>
        <tbody>
          {paths.map((p) => (
            <tr key={p.purpose} className="border-t align-top">
              <td className="py-2 pr-3">
                <div className="font-mono font-medium text-fg">{p.purpose}</div>
                <div className="text-fg-muted mt-0.5 max-w-[16rem]">{p.description}</div>
              </td>
              <td className="py-2 pr-3 whitespace-nowrap">
                <span className="inline-flex items-center gap-1.5">
                  <Dot tone={p.enabled ? "ok" : "off"} />
                  {p.enabled ? "enabled" : "disabled"}
                </span>
                <div className="text-fg-faint mt-0.5 max-w-[14rem] whitespace-normal">{p.enabled ? p.state : p.disabled_reason}</div>
              </td>
              <td className="py-2 pr-3">
                <code className="break-all">{p.destination}</code>
                <div className="mt-0.5">
                  <Pill tone={p.destination_class === "public" ? "warn" : "ok"}>{p.destination_class}</Pill>
                </div>
              </td>
              <td className="py-2 pr-3 text-fg-muted max-w-[18rem]">
                {p.data}
                <div className="mt-0.5">{p.carries_note_content ? <Pill tone="warn">carries note content</Pill> : <Pill tone="ok">no note content</Pill>}</div>
              </td>
              <td className="py-2 pr-3 font-mono">{p.permitted_by.join(" ")}</td>
              <td className="py-2 pr-3 tnum">{num(p.uses_total)}</td>
              <td className="py-2 pr-3 tnum">{bytes(p.bytes_out_total)}</td>
              <td className="py-2 text-fg-muted whitespace-nowrap" title={absTime(p.last_used)}>
                {p.last_used ? relTime(p.last_used) : "never"}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function AuditSection() {
  const [action, setAction] = useState<AuditActionFilter | "">("");
  const [actor, setActor] = useState("");
  const [q, setQ] = useState("");
  const [rows, setRows] = useState<AuditRow[]>([]);
  const [total, setTotal] = useState(0);
  const [next, setNext] = useState<number | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<ReturnType<typeof toApiError> | null>(null);
  const toast = useToast();

  const load = (before?: number) => {
    setLoading(true);
    const p: Parameters<typeof api.audit>[0] = { limit: 50 };
    if (before !== undefined) p.before = before;
    if (action) p.action = action;
    if (actor) p.actor = actor;
    if (q) p.q = q;
    api.audit(p).then(
      (page) => {
        setRows((r) => (before === undefined ? page.rows : [...r, ...page.rows]));
        setTotal(page.total);
        setNext(page.next_before);
        setLoading(false);
        setError(null);
      },
      (e) => {
        // A 400 here is the server refusing a filter that matches nothing, with the names
        // that exist; keep the old rows off the screen so the message is not read as data.
        setError(toApiError(e));
        if (before === undefined) {
          setRows([]);
          setTotal(0);
          setNext(null);
        }
        setLoading(false);
      },
    );
  };
  useEffect(() => {
    const t = setTimeout(() => load(), 150);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [action, actor, q]);

  const exportJson = async () => {
    // The export is built in the page and handed to the browser; nothing is uploaded.
    try {
      const all: AuditRow[] = [];
      let before: number | undefined;
      for (let i = 0; i < 100; i++) {
        const p: Parameters<typeof api.audit>[0] = { limit: 1000 };
        if (before !== undefined) p.before = before;
        const page = await api.audit(p);
        all.push(...page.rows);
        if (page.next_before === null) break;
        before = page.next_before;
      }
      const blob = new Blob([JSON.stringify(all, null, 2)], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `cyberbrain-audit-${new Date().toISOString().slice(0, 10)}.json`;
      a.click();
      URL.revokeObjectURL(url);
      toast(`exported ${all.length} rows`);
    } catch (e) {
      toast(toApiError(e).message, "err");
    }
  };

  const toneOf = (a: AuditAction): "ok" | "warn" | "danger" | "neutral" => {
    if (a === "policy.refusal" || a === "egress.failed" || a === "egress.abandoned" || a === "note.erase.failed") return "danger";
    if (a.startsWith("note.erase") || a === "retention.expired" || a === "note.write.held" || a === "index.cleared" || a === "index.note-dropped") return "warn";
    if (a === "inference.call" || a === "egress.completed" || a === "egress.permitted") return "ok";
    return "neutral";
  };

  return (
    <Section
      title="Audit log"
      aside={
        <>
          <span className="tnum">{num(total)} rows match</span>
          <button className="btn btn-sm" onClick={exportJson}>
            export JSON
          </button>
        </>
      }
    >
      <div className="flex flex-wrap gap-2 mb-3">
        <select className="input" value={action} onChange={(e) => setAction(e.target.value as AuditActionFilter | "")} aria-label="Action family" title="Filters by family prefix: note selects note.write, note.erase and their sub-actions">
          <option value="">any action</option>
          {ACTION_FILTERS.map((a) => (
            <option key={a.value} value={a.value}>
              {a.label}
            </option>
          ))}
        </select>
        <input className="input w-40" placeholder="actor prefix" value={actor} onChange={(e) => setActor(e.target.value)} aria-label="Actor" />
        <input className="input flex-1 min-w-40" placeholder="subject or detail contains…" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Search audit" />
      </div>
      {error ? <ErrorBanner error={error} onRetry={() => load()} /> : null}
      <div className="overflow-x-auto">
        <table className="w-full text-xs min-w-[44rem]">
          <thead>
            <tr className="label text-left">
              <th className="py-1 pr-3 font-medium tnum">seq</th>
              <th className="py-1 pr-3 font-medium">time</th>
              <th className="py-1 pr-3 font-medium">actor</th>
              <th className="py-1 pr-3 font-medium">action</th>
              <th className="py-1 pr-3 font-medium">subject</th>
              <th className="py-1 font-medium">detail</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.seq} className="border-t align-top">
                <td className="py-1 pr-3 font-mono tnum text-fg-faint">{r.seq}</td>
                <td className="py-1 pr-3 font-mono tnum whitespace-nowrap text-fg-muted" title={relTime(r.ts)}>
                  {absTime(r.ts)}
                </td>
                <td className="py-1 pr-3 font-mono whitespace-nowrap">{r.actor}</td>
                <td className="py-1 pr-3">
                  <Pill tone={toneOf(r.action)}>{r.action}</Pill>
                </td>
                <td className="py-1 pr-3 font-mono break-all max-w-[16rem]">{r.subject}</td>
                <td className="py-1 text-fg-muted">
                  {Object.keys(r.detail).length ? (
                    Object.entries(r.detail).map(([k, v]) => (
                      <span key={k} className="inline-block mr-2 whitespace-nowrap" title={typeof v === "string" && /^[[{]/.test(v) ? "nested value, shown as its JSON text" : undefined}>
                        <span className="text-fg-faint">{k}</span>=<span className="font-mono">{v === null ? "null" : String(v)}</span>
                      </span>
                    ))
                  ) : (
                    <span className="text-fg-faint">no detail on this row</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!loading && !rows.length ? <Empty title="No audit rows match." /> : null}
      {loading ? <Loading /> : null}
      {next !== null && !loading ? (
        <div className="mt-2 text-center">
          <button className="btn btn-sm" onClick={() => load(next)}>
            older rows
          </button>
        </div>
      ) : null}
    </Section>
  );
}

function RetentionSection() {
  const queue = useAsync(() => api.retention(), []);
  const [report, setReport] = useState<RetentionApplyReport | null>(null);
  const [busy, setBusy] = useState(false);
  const toast = useToast();
  const run = async (dryRun: boolean) => {
    setBusy(true);
    try {
      const r = await api.applyRetention(dryRun);
      setReport(r);
      if (!dryRun) {
        toast(`erased ${r.removed.length} note${r.removed.length === 1 ? "" : "s"} through the forget path`);
        queue.reload();
      }
    } catch (e) {
      toast(toApiError(e).message, "err");
    } finally {
      setBusy(false);
    }
  };
  const entries = queue.data?.entries ?? [];
  const due = queue.data?.due ?? 0;
  const invalid = entries.filter((e) => e.invalid !== undefined).length;
  return (
    <Section
      title="Retention queue"
      aside={
        <>
          <span className="tnum">
            {entries.length} with a retention · <span className={due ? "text-warn" : ""}>{due} due</span>
            {invalid ? <> · <span className="text-warn">{invalid} invalid</span></> : null}
            {queue.data ? <> · {queue.data.indefinite} kept indefinitely</> : null}
          </span>
          <button className="btn btn-sm" disabled={!due || busy} onClick={() => run(true)} title="Runs the real erase path with a no-op writer and shows what would go">
            dry-run apply
          </button>
        </>
      }
    >
      {queue.error ? <ErrorBanner error={queue.error} onRetry={queue.reload} /> : null}
      {queue.data ? (
        entries.length ? (
          <table className="w-full text-xs">
            <thead>
              <tr className="label text-left">
                <th className="py-1 pr-3 font-medium">note</th>
                <th className="py-1 pr-3 font-medium">retention</th>
                <th className="py-1 pr-3 font-medium">expires</th>
                <th className="py-1 font-medium">state</th>
              </tr>
            </thead>
            <tbody>
              {entries.slice(0, 40).map((e) => (
                <tr key={e.note.id} className="border-t">
                  <td className="py-1.5 pr-3">
                    <span className="inline-flex items-center gap-1.5">
                      <RingBadge ring={e.note.ring} />
                      <a href={href("note", e.note.name)} className="link font-mono">
                        {e.note.name}
                      </a>
                    </span>
                  </td>
                  <td className="py-1.5 pr-3">
                    {e.invalid !== undefined ? <code className="text-warn" title={e.invalid}>{e.retention}</code> : <span title={e.retention}>{duration(e.retention)}</span>}
                  </td>
                  <td className="py-1.5 pr-3 tnum text-fg-muted" title={e.invalid !== undefined ? "No expiry can be computed from an invalid duration." : absTime(e.expires_at)}>
                    {e.invalid !== undefined ? <span className="text-fg-faint">never</span> : relTime(e.expires_at)}
                  </td>
                  <td className="py-1.5">
                    {e.invalid !== undefined ? (
                      <Pill tone="warn" title={e.invalid}>
                        invalid: {e.invalid}
                      </Pill>
                    ) : e.due ? (
                      <Pill tone="warn">due — awaiting apply</Pill>
                    ) : (
                      <Pill>kept</Pill>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <Empty title={`No note carries a retention duration; ${queue.data ? `all ${num(queue.data.indefinite)} notes are` : "everything is"} kept indefinitely.`} />
        )
      ) : null}
      {entries.length > 40 ? <div className="mt-2 text-2xs text-fg-faint">{entries.length - 40} more, sorted by expiry</div> : null}
      <p className="mt-3 text-xs text-fg-muted">Expiry never happens in the background. A due note stays until you apply, and applying goes through the same path as <code>forget</code>: file, blocks, vectors, FTS rows and links in one transaction, one audit row each.</p>
      {report ? (
        <div className="mt-3 panel px-3 py-2 text-xs">
          <div className="flex items-center gap-2">
            <Pill tone={report.dry_run ? "neutral" : "warn"}>{report.dry_run ? "dry run" : "applied"}</Pill>
            <span>
              {report.removed.length} note{report.removed.length === 1 ? "" : "s"} {report.dry_run ? "would be" : ""} erased
            </span>
            {report.dry_run && report.removed.length ? (
              <button className="btn btn-sm btn-danger ml-auto" disabled={busy} onClick={() => run(false)}>
                apply for real
              </button>
            ) : (
              <button className="btn btn-sm ml-auto" onClick={() => setReport(null)}>
                close
              </button>
            )}
          </div>
          <ul className="mt-1.5 font-mono tnum space-y-0.5">
            {report.removed.map((r) => (
              <li key={r.note.id}>
                {r.note.name}: {r.removed.blocks} blocks, {r.removed.vectors} vectors, {r.removed.fts_rows} fts, {r.removed.links_in + r.removed.links_out} links
                {r.notes.length ? <span className="text-fg-faint font-sans"> · {r.notes.join("; ")}</span> : null}
              </li>
            ))}
          </ul>
          {report.skipped.length ? (
            <div className="mt-2">
              <div className="label">skipped, with the reason</div>
              <ul className="mt-0.5 space-y-0.5">
                {report.skipped.map((x) => (
                  <li key={x.name}>
                    <span className="font-mono">{x.name}</span> <span className="text-fg-muted">— {x.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          ) : null}
        </div>
      ) : null}
    </Section>
  );
}

function SubjectSection() {
  const [q, setQ] = useState("");
  const [report, setReport] = useState<SubjectAccessReport | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ReturnType<typeof toApiError> | null>(null);
  const run = async () => {
    if (!q.trim()) return;
    setBusy(true);
    setError(null);
    try {
      setReport(await api.subjectAccess(q.trim()));
    } catch (e) {
      setError(toApiError(e));
    } finally {
      setBusy(false);
    }
  };
  const grouped = useMemo(() => {
    if (!report) return null;
    const g = { note: [] as SubjectAccessReport["hits"], block: [] as SubjectAccessReport["hits"], audit: [] as SubjectAccessReport["hits"] };
    for (const h of report.hits) g[h.where].push(h);
    return g;
  }, [report]);
  return (
    <Section title="Subject access request" aside={<span>GDPR Art. 15 · FADP Art. 25</span>}>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          run();
        }}
      >
        <input className="input flex-1" placeholder="name, e-mail or handle" value={q} onChange={(e) => setQ(e.target.value)} aria-label="Identifier" />
        <button className="btn" disabled={busy || !q.trim()}>
          {busy ? "searching…" : "search everything"}
        </button>
      </form>
      <p className="mt-2 text-xs text-fg-muted">Searches every note, block and audit row for the identifier and lists what is held, with citations, in a form that can be handed to the person. The search itself writes an audit row.</p>
      {error ? <div className="mt-3"><ErrorBanner error={error} /></div> : null}
      {report && grouped ? (
        <div className="mt-3 text-xs">
          <div className="text-fg-muted tnum">
            {report.hits.length} hit{report.hits.length === 1 ? "" : "s"} for <code className="text-fg">{report.identifier}</code> across {num(report.searched.notes)} notes, {num(report.searched.blocks)} blocks, {num(report.searched.audit_rows)} audit rows
          </div>
          <div className="mt-1 text-fg-muted">
            response due: <span className="text-fg">{report.response_deadline}</span>
          </div>
          {report.caveats.length ? (
            <ul className="mt-1.5 space-y-0.5 text-fg-faint">
              {report.caveats.map((c, i) => (
                <li key={i}>· {c}</li>
              ))}
            </ul>
          ) : null}
          {report.hits.length === 0 ? (
            <div className="mt-2 panel px-3 py-2 text-ok">Nothing found for this identifier in what was searched; read the caveats above for what a substring search cannot see.</div>
          ) : (
            <ul className="mt-2 space-y-1">
              {report.hits.map((h, i) => (
                <li key={i} className="panel px-3 py-1.5 flex items-start gap-2">
                  <Pill>{h.where}</Pill>
                  {h.citation ? <CitationChip citation={h.citation} size="sm" /> : <code className="text-fg-muted">{h.ref}</code>}
                  <span className="text-fg-muted min-w-0 break-words">{h.excerpt}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      ) : null}
    </Section>
  );
}
