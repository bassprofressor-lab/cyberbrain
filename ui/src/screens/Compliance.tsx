import { useEffect, useMemo, useRef, useState } from "react";
import { api, AUDIT_ACTION_FAMILIES, type AuditAction, type AuditActionFilter, type AuditRow, type EgressPath, type PiiState, type RetentionApplyReport, type SubjectAccessReport } from "@/api/client";
import { CitationChip } from "@/components/Citation";
import { RingBadge } from "@/components/RingBadge";
import { useToast } from "@/components/Toast";
import { Dot, Empty, ErrorBanner, KeyValue, Loading, Pill, Section, Stat } from "@/components/ui";
import { absTime, bytes, duration, num, relTime, shortDate } from "@/lib/format";
import { useT } from "@/lib/i18n";
import { useShortcuts } from "@/lib/keys";
import { href, type Route } from "@/lib/router";
import { toApiError, useAsync } from "@/lib/useAsync";

/** Anchors, so they never change with the language; the label comes from the dictionary. */
const SECTIONS = ["overview", "egress", "audit", "pii", "retention", "models", "subject"] as const;

/** The log's own vocabulary, by family: `note` selects `note.*`, and so on (`AuditActionFilter`). */
const ACTION_FILTERS = AUDIT_ACTION_FAMILIES;

function piiTone(state: PiiState): "ok" | "warn" | "danger" | "neutral" {
  return state === "flagged" ? "danger" : state === "reviewed" ? "warn" : state === "none" ? "ok" : "neutral";
}
export function ComplianceScreen({ route }: { route: Route }) {
  const t = useT();
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
    for (const id of SECTIONS) {
      const el = document.getElementById(`c-${id}`);
      if (el) obs.observe(el);
    }
    return () => obs.disconnect();
  }, [egress.data, pii.data, retention.data, models.data]);

  useShortcuts(
    "compliance",
    SECTIONS.map((id, i) => ({ keys: String(i + 1), label: t.compliance.jumpTo(t.compliance.sections[id]), run: () => document.getElementById(`c-${id}`)?.scrollIntoView({ block: "start" }) })),
    [t],
  );

  const profile = egress.data?.profile ?? status.data?.policy.profile;

  return (
    <div className="grid h-full grid-cols-1 md:grid-cols-[12rem_1fr]">
      <nav className="border-r hidden md:block py-3">
        <ul className="text-sm">
          {SECTIONS.map((id, i) => (
            <li key={id}>
              <a href={href("compliance", null, undefined, id)} className={`flex items-center gap-2 px-4 py-1.5 ${active === id ? "row-selected font-medium" : "text-fg-muted hover:text-fg"}`}>
                <span className="font-mono text-2xs text-fg-faint w-3">{i + 1}</span>
                {t.compliance.sections[id]}
              </a>
            </li>
          ))}
        </ul>
        {profile ? (
          <div className="px-4 mt-4 text-2xs text-fg-faint">
            {t.compliance.profile} <code className={profile === "off" ? "text-warn" : "text-fg"}>{profile}</code>
            <div className="mt-1 leading-relaxed">{profile === "eu" ? t.compliance.profileEu : profile === "ch" ? t.compliance.profileCh : t.compliance.profileOff}</div>
          </div>
        ) : null}
      </nav>
      <div ref={scrollRef} className="overflow-auto scroll-thin p-5 space-y-5 min-w-0">
        <div id="c-overview">
          {egress.error ? <ErrorBanner error={egress.error} onRetry={egress.reload} /> : egress.data ? <Overview register={egress.data} auditRows={status.data?.policy.audit_rows ?? null} /> : <Loading label={t.compliance.loadingEgress} />}
        </div>

        <Section id="c-egress" title={t.compliance.sections.egress} aside={egress.data ? <span className="font-mono">{t.compliance.egress.register(egress.data.register_hash)}</span> : null}>
          {egress.data ? <EgressTable paths={egress.data.paths} /> : null}
          <p className="mt-3 text-xs text-fg-muted leading-relaxed">{t.compliance.egress.note}</p>
        </Section>

        <div id="c-audit">
          <AuditSection />
        </div>

        <Section id="c-pii" title={t.compliance.pii.title} aside={pii.data ? <Pill tone={pii.data.scan_enabled ? "ok" : "warn"}>{pii.data.scan_enabled ? t.compliance.pii.scanOn : t.compliance.pii.scanOff}</Pill> : null}>
          {pii.error ? <ErrorBanner error={pii.error} onRetry={pii.reload} /> : null}
          {pii.data ? (
            <>
              {pii.data.holds.length ? (
                <div className="mb-3 panel border-warn/50 px-3 py-2 text-sm">
                  <span className="text-warn font-medium">{t.compliance.pii.held(pii.data.holds.length)}</span> {t.compliance.pii.awaiting}{" "}
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
                      <th className="py-1 pr-3 font-medium">{t.compliance.pii.colNote}</th>
                      <th className="py-1 pr-3 font-medium">{t.compliance.pii.colState}</th>
                      <th className="py-1 pr-3 font-medium">{t.compliance.pii.colFindings}</th>
                      <th className="py-1 font-medium">{t.compliance.pii.colReviewed}</th>
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
                          <Pill tone={piiTone(e.state)} title={e.state === "unscanned" ? t.compliance.pii.unscannedTitle : undefined}>
                            {t.pii.state[e.state]}
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
                            <span className="text-fg-faint font-sans">{e.state === "unscanned" ? t.compliance.pii.unscannedNothing : t.compliance.pii.nothingNow}</span>
                          )}
                        </td>
                        <td className="py-1.5 text-fg-muted" title={absTime(e.reviewed_at)}>
                          {e.reviewed_at ? relTime(e.reviewed_at) : <span className="text-fg-faint">{t.compliance.pii.neverScanned}</span>}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              ) : (
                <Empty title={t.compliance.pii.emptyTitle}>{t.compliance.pii.emptyBody}</Empty>
              )}
            </>
          ) : null}
        </Section>

        <div id="c-retention">
          <RetentionSection />
        </div>

        <Section id="c-models" title={t.compliance.sections.models} aside={<span>{t.compliance.models.aside}</span>}>
          {models.error ? <ErrorBanner error={models.error} onRetry={models.reload} /> : null}
          {models.data ? (
            <div className="grid gap-3 lg:grid-cols-2">
              {models.data.map((m) => (
                <div key={m.role + m.name} className="panel p-3 text-sm">
                  <div className="flex items-center gap-2">
                    <Pill tone={m.active ? "ok" : "neutral"}>{m.role}</Pill>
                    <span className="font-mono font-medium">{m.name}</span>
                    {m.active ? <span className="ml-auto text-2xs text-fg-faint">{t.compliance.models.inUse}</span> : null}
                  </div>
                  <div className="mt-2">
                    <KeyValue
                      rows={[
                        [t.compliance.models.source, <span className="text-xs">{m.source}</span>],
                        [t.compliance.models.licence, m.license === "not stated" ? <span className="text-fg-muted">{t.compliance.models.licenceNone}</span> : m.license],
                        [t.compliance.models.hash, m.hash ? <code className="text-xs break-all">{m.hash}</code> : <span className="text-fg-faint">{t.compliance.models.hashNone}</span>],
                        [t.compliance.models.format, m.format ?? <span className="text-fg-faint">{t.common.notStated}</span>],
                        [t.compliance.models.dimension, m.dim ? `${m.dim} · ${m.pooling ?? t.compliance.models.poolingNone}` : <span className="text-fg-faint">{m.role === "inference" ? t.compliance.models.dimNotApplicable : t.common.notStated}</span>],
                        [t.compliance.models.size, m.bytes === null ? <span className="text-fg-faint">{m.role === "inference" ? t.compliance.models.notHeld : t.common.notStated}</span> : bytes(m.bytes)],
                        [t.compliance.models.verified, m.verified_at ? <span title={absTime(m.verified_at)}>{t.compliance.models.verifiedOnLoad(relTime(m.verified_at))}</span> : m.hash ? <span className="text-fg-muted">{t.compliance.models.verifiedNoStamp}</span> : <span className="text-fg-faint">{t.compliance.models.nothingToVerify}</span>],
                      ]}
                    />
                  </div>
                  <div className="mt-2 text-xs">
                    <div className="label">{t.compliance.models.intendedUse}</div>
                    <p className="mt-0.5 text-fg-muted leading-relaxed">{m.intended_use}</p>
                    <div className="label mt-2">{t.compliance.models.limitations}</div>
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
  const t = useT();
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
    headline = t.compliance.overview.nothingLeft;
    sub = t.compliance.overview.nothingLeftSub(days, pubEnabled.length);
  } else if (pub.every((p) => p.purpose === "model-download")) {
    headline = t.compliance.overview.noContent;
    sub = t.compliance.overview.noContentSub(days, pubUses, bytes(pubBytes), relTime(lastPub), pubEnabled.length === 0);
  } else {
    tone = "warn";
    headline = t.compliance.overview.bytesLeft;
    sub = t.compliance.overview.bytesLeftSub(pubUses, days, bytes(pubBytes));
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
          <div>{t.compliance.profile} <code className="text-fg">{register.profile}</code></div>
          <div title={absTime(register.since)}>{t.compliance.overview.since(shortDate(register.since))}</div>
        </div>
      </div>
      <div className="mt-4 grid gap-2 sm:grid-cols-2 lg:grid-cols-4">
        <Stat label={t.compliance.overview.internet} value={pubUses === 0 ? t.compliance.overview.noRequests : t.compliance.overview.requests(num(pubUses))} sub={pubUses === 0 ? t.compliance.overview.noBytes : t.compliance.overview.bytesOut(bytes(pubBytes), pub.map((p) => p.purpose).join(", "))} tone={pubUses === 0 ? "ok" : undefined} />
        <Stat label={t.compliance.overview.ownNetwork} value={t.compliance.overview.calls(num(localUses))} sub={local.length ? `${local.map((p) => p.destination.replace(/^https?:\/\//, "")).join(", ")} · ${local.map((p) => p.destination_class).join(", ")}` : t.compliance.overview.noLocalEndpoint} />
        <Stat label={t.compliance.overview.refused} value={num(register.refused_total)} sub={register.refused_total ? t.compliance.overview.refusedSub : t.compliance.overview.refusedNone} tone={register.refused_total ? "warn" : "ok"} />
        <Stat label={t.compliance.overview.auditRows} value={auditRows === null ? "—" : num(auditRows)} sub={t.compliance.overview.auditRowsSub} />
      </div>
      <div className="mt-4 grid gap-x-6 gap-y-1.5 sm:grid-cols-2 text-xs text-fg-muted">
        <div className="label sm:col-span-2">{t.compliance.overview.howKnown}</div>
        <div>· {t.compliance.overview.how1(register.register_hash)}</div>
        <div>· {t.compliance.overview.how2}</div>
        <div>· {t.compliance.overview.how3}</div>
        <div>· {t.compliance.overview.how4}</div>
        <div>· {t.compliance.overview.how5}</div>
        <div>· {t.compliance.overview.how6}</div>
      </div>
    </div>
  );
}

function EgressTable({ paths }: { paths: EgressPath[] }) {
  const t = useT();
  return (
    <div className="overflow-x-auto">
      <table className="w-full text-xs min-w-[48rem]">
        <thead>
          <tr className="label text-left">
            <th className="py-1 pr-3 font-medium">{t.compliance.egress.colPurpose}</th>
            <th className="py-1 pr-3 font-medium">{t.compliance.egress.colState}</th>
            <th className="py-1 pr-3 font-medium">{t.compliance.egress.colDestination}</th>
            <th className="py-1 pr-3 font-medium">{t.compliance.egress.colData}</th>
            <th className="py-1 pr-3 font-medium">{t.compliance.egress.colPermitted}</th>
            <th className="py-1 pr-3 font-medium tnum">{t.compliance.egress.colUses}</th>
            <th className="py-1 pr-3 font-medium tnum">{t.compliance.egress.colBytes}</th>
            <th className="py-1 font-medium">{t.compliance.egress.colLast}</th>
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
                  {p.enabled ? t.compliance.egress.enabled : t.compliance.egress.disabled}
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
                <div className="mt-0.5">{p.carries_note_content ? <Pill tone="warn">{t.compliance.egress.carries}</Pill> : <Pill tone="ok">{t.compliance.egress.carriesNot}</Pill>}</div>
              </td>
              <td className="py-2 pr-3 font-mono">{p.permitted_by.join(" ")}</td>
              <td className="py-2 pr-3 tnum">{num(p.uses_total)}</td>
              <td className="py-2 pr-3 tnum">{bytes(p.bytes_out_total)}</td>
              <td className="py-2 text-fg-muted whitespace-nowrap" title={absTime(p.last_used)}>
                {p.last_used ? relTime(p.last_used) : t.common.never}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function AuditSection() {
  const t = useT();
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
    const timer = setTimeout(() => load(), 150);
    return () => clearTimeout(timer);
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
      toast(t.compliance.audit.exported(all.length));
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
      title={t.compliance.audit.title}
      aside={
        <>
          <span className="tnum">{t.compliance.audit.rowsMatch(num(total))}</span>
          <button className="btn btn-sm" onClick={exportJson}>
            {t.compliance.audit.export}
          </button>
        </>
      }
    >
      <div className="flex flex-wrap gap-2 mb-3">
        <select className="input" value={action} onChange={(e) => setAction(e.target.value as AuditActionFilter | "")} aria-label={t.compliance.audit.actionAria} title={t.compliance.audit.actionTitle}>
          <option value="">{t.compliance.audit.anyAction}</option>
          {ACTION_FILTERS.map((a) => (
            <option key={a} value={a}>
              {t.compliance.audit.families[a as keyof typeof t.compliance.audit.families] ?? a}
            </option>
          ))}
        </select>
        <input className="input w-40" placeholder={t.compliance.audit.actorPlaceholder} value={actor} onChange={(e) => setActor(e.target.value)} aria-label={t.compliance.audit.actorAria} />
        <input className="input flex-1 min-w-40" placeholder={t.compliance.audit.searchPlaceholder} value={q} onChange={(e) => setQ(e.target.value)} aria-label={t.compliance.audit.searchAria} />
      </div>
      {error ? <ErrorBanner error={error} onRetry={() => load()} /> : null}
      <div className="overflow-x-auto">
        <table className="w-full text-xs min-w-[44rem]">
          <thead>
            <tr className="label text-left">
              <th className="py-1 pr-3 font-medium tnum">{t.compliance.audit.colSeq}</th>
              <th className="py-1 pr-3 font-medium">{t.compliance.audit.colTime}</th>
              <th className="py-1 pr-3 font-medium">{t.compliance.audit.colActor}</th>
              <th className="py-1 pr-3 font-medium">{t.compliance.audit.colAction}</th>
              <th className="py-1 pr-3 font-medium">{t.compliance.audit.colSubject}</th>
              <th className="py-1 font-medium">{t.compliance.audit.colDetail}</th>
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
                      <span key={k} className="inline-block mr-2 whitespace-nowrap" title={typeof v === "string" && /^[[{]/.test(v) ? t.compliance.audit.nestedTitle : undefined}>
                        <span className="text-fg-faint">{k}</span>=<span className="font-mono">{v === null ? "null" : String(v)}</span>
                      </span>
                    ))
                  ) : (
                    <span className="text-fg-faint">{t.compliance.audit.noDetail}</span>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      {!loading && !rows.length ? <Empty title={t.compliance.audit.empty} /> : null}
      {loading ? <Loading /> : null}
      {next !== null && !loading ? (
        <div className="mt-2 text-center">
          <button className="btn btn-sm" onClick={() => load(next)}>
            {t.compliance.audit.older}
          </button>
        </div>
      ) : null}
    </Section>
  );
}

function RetentionSection() {
  const t = useT();
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
        toast(t.compliance.retention.erasedToast(r.removed.length));
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
      title={t.compliance.retention.title}
      aside={
        <>
          <span className="tnum">
            <span className={due ? "text-warn" : ""}>{t.compliance.retention.summary(entries.length, due)}</span>
            {invalid ? <> · <span className="text-warn">{t.compliance.retention.invalid(invalid)}</span></> : null}
            {queue.data ? <> · {t.compliance.retention.indefinite(queue.data.indefinite)}</> : null}
          </span>
          <button className="btn btn-sm" disabled={!due || busy} onClick={() => run(true)} title={t.compliance.retention.dryRunTitle}>
            {t.compliance.retention.dryRun}
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
                <th className="py-1 pr-3 font-medium">{t.compliance.retention.colNote}</th>
                <th className="py-1 pr-3 font-medium">{t.compliance.retention.colRetention}</th>
                <th className="py-1 pr-3 font-medium">{t.compliance.retention.colExpires}</th>
                <th className="py-1 font-medium">{t.compliance.retention.colState}</th>
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
                  <td className="py-1.5 pr-3 tnum text-fg-muted" title={e.invalid !== undefined ? t.compliance.retention.invalidTitle : absTime(e.expires_at)}>
                    {e.invalid !== undefined ? <span className="text-fg-faint">{t.common.never}</span> : relTime(e.expires_at)}
                  </td>
                  <td className="py-1.5">
                    {e.invalid !== undefined ? (
                      <Pill tone="warn" title={e.invalid}>
                        {t.compliance.retention.invalidPill(e.invalid)}
                      </Pill>
                    ) : e.due ? (
                      <Pill tone="warn">{t.compliance.retention.due}</Pill>
                    ) : (
                      <Pill>{t.compliance.retention.kept}</Pill>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : (
          <Empty title={t.compliance.retention.emptyTitle(queue.data ? num(queue.data.indefinite) : null)} />
        )
      ) : null}
      {entries.length > 40 ? <div className="mt-2 text-2xs text-fg-faint">{t.compliance.retention.more(entries.length - 40)}</div> : null}
      <p className="mt-3 text-xs text-fg-muted">{t.compliance.retention.note}</p>
      {report ? (
        <div className="mt-3 panel px-3 py-2 text-xs">
          <div className="flex items-center gap-2">
            <Pill tone={report.dry_run ? "neutral" : "warn"}>{report.dry_run ? t.note.forgetDialog.dryRun : t.compliance.retention.applied}</Pill>
            <span>{report.dry_run ? t.compliance.retention.wouldErase(report.removed.length) : t.compliance.retention.erased(report.removed.length)}</span>
            {report.dry_run && report.removed.length ? (
              <button className="btn btn-sm btn-danger ml-auto" disabled={busy} onClick={() => run(false)}>
                {t.compliance.retention.applyReal}
              </button>
            ) : (
              <button className="btn btn-sm ml-auto" onClick={() => setReport(null)}>
                {t.common.close}
              </button>
            )}
          </div>
          <ul className="mt-1.5 font-mono tnum space-y-0.5">
            {report.removed.map((r) => (
              <li key={r.note.id}>
                {r.note.name}: {t.compliance.retention.removedLine({ blocks: r.removed.blocks, vectors: r.removed.vectors, fts: r.removed.fts_rows, links: r.removed.links_in + r.removed.links_out })}
                {r.notes.length ? <span className="text-fg-faint font-sans"> · {r.notes.join("; ")}</span> : null}
              </li>
            ))}
          </ul>
          {report.skipped.length ? (
            <div className="mt-2">
              <div className="label">{t.compliance.retention.skipped}</div>
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
  const t = useT();
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
    <Section title={t.compliance.subject.title} aside={<span>{t.compliance.subject.aside}</span>}>
      <form
        className="flex gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          run();
        }}
      >
        <input className="input flex-1" placeholder={t.compliance.subject.placeholder} value={q} onChange={(e) => setQ(e.target.value)} aria-label={t.compliance.subject.aria} />
        <button className="btn" disabled={busy || !q.trim()}>
          {busy ? t.common.searching : t.compliance.subject.submit}
        </button>
      </form>
      <p className="mt-2 text-xs text-fg-muted">{t.compliance.subject.note}</p>
      {error ? <div className="mt-3"><ErrorBanner error={error} /></div> : null}
      {report && grouped ? (
        <div className="mt-3 text-xs">
          <div className="text-fg-muted tnum">
            {t.compliance.subject.hits(report.hits.length, report.identifier, num(report.searched.notes), num(report.searched.blocks), num(report.searched.audit_rows))}
          </div>
          <div className="mt-1 text-fg-muted">
            {t.compliance.subject.deadline} <span className="text-fg">{report.response_deadline}</span>
          </div>
          {report.caveats.length ? (
            <ul className="mt-1.5 space-y-0.5 text-fg-faint">
              {report.caveats.map((c, i) => (
                <li key={i}>· {c}</li>
              ))}
            </ul>
          ) : null}
          {report.hits.length === 0 ? (
            <div className="mt-2 panel px-3 py-2 text-ok">{t.compliance.subject.nothing}</div>
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
