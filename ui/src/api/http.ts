/**
 * The real transport. Every route in `types.ts`, nothing else. Same origin, no cookies, no
 * external host: `connect-src 'self'` in the CSP makes anything else fail loudly.
 */
import {
  ApiError,
  type ApiErrorBody,
  type AuditPage,
  type HubStatus,
  type AuditParams,
  type CitationExpansion,
  type CommandResult,
  type CyberbrainApi,
  type DoctorReport,
  type EgressRegister,
  type ForgetReport,
  type Graph,
  type ModelCard,
  type NoteDetail,
  type NoteListParams,
  type NoteSummary,
  type NoteWriteRequest,
  type ObligationsView,
  type PiiHoldResolution,
  type PiiReport,
  type RecallParams,
  type RecallResult,
  type RetentionApplyReport,
  type RetentionQueue,
  type ScanReport,
  type StatusReport,
  type SubjectAccessReport,
  type UsageReport,
} from "./types";

const BASE = "/api/v1";

function qs(params: Record<string, string | number | boolean | undefined | null>): string {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v === undefined || v === null || v === "") continue;
    p.set(k, String(v));
  }
  const s = p.toString();
  return s ? `?${s}` : "";
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = {
    method,
    headers: { Accept: "application/json" },
    // Belt and braces with the CSP: never send credentials, never follow to another origin.
    credentials: "omit",
    redirect: "error",
    cache: "no-store",
  };
  if (body !== undefined) {
    init.headers = { ...init.headers, "Content-Type": "application/json" };
    init.body = JSON.stringify(body);
  }
  let res: Response;
  try {
    res = await fetch(BASE + path, init);
  } catch (e) {
    throw new ApiError(0, {
      code: "internal",
      message: `cyberbrain serve is not reachable (${e instanceof Error ? e.message : String(e)})`,
      exit_code: 2,
    });
  }
  if (res.status === 204) return null as T;
  const text = await res.text();
  let json: unknown = null;
  if (text) {
    try {
      json = JSON.parse(text);
    } catch {
      throw new ApiError(res.status, { code: "internal", message: `non-JSON response (${res.status}) from ${path}`, exit_code: 2 });
    }
  }
  if (!res.ok) {
    const err = (json as ApiErrorBody | null)?.error;
    throw new ApiError(
      res.status,
      err ?? { code: res.status === 404 ? "not-found" : res.status >= 500 ? "internal" : "bad-request", message: `HTTP ${res.status} from ${path}`, exit_code: res.status >= 500 ? 2 : 1 },
    );
  }
  return json as T;
}

export const httpClient: CyberbrainApi = {
  transport: "http",

  status: () => request<StatusReport>("GET", "/status"),
  hubStatus: () => request<HubStatus>("GET", "/hub"),
  usage: (days) => request<UsageReport>("GET", `/usage${qs({ days })}`),
  recall: (p: RecallParams) => request<RecallResult>("GET", `/recall${qs({ q: p.q, n: p.n, ring: p.ring })}`),
  expand: (citation) => request<CitationExpansion>("GET", `/recall/${encodeURIComponent(citation)}`),

  listNotes: (p: NoteListParams = {}) => request<NoteSummary[]>("GET", `/notes${qs({ ring: p.ring, kind: p.kind, q: p.q, sort: p.sort })}`),
  getNote: (nameOrId) => request<NoteDetail>("GET", `/notes/${encodeURIComponent(nameOrId)}`),
  writeNote: (nameOrId, req: NoteWriteRequest) => request<NoteDetail>("PUT", `/notes/${encodeURIComponent(nameOrId)}`, req),
  resolveHold: (holdId, res: PiiHoldResolution) => request<NoteDetail | null>("POST", `/holds/${encodeURIComponent(holdId)}`, res),
  forget: (nameOrId, dryRun) => request<ForgetReport>("DELETE", `/notes/${encodeURIComponent(nameOrId)}${qs({ dry_run: dryRun })}`),

  graph: () => request<Graph>("GET", "/graph"),

  egress: () => request<EgressRegister>("GET", "/policy/egress"),
  obligations: () => request<ObligationsView>("GET", "/policy/obligations"),
  audit: (p: AuditParams = {}) => request<AuditPage>("GET", `/policy/audit${qs({ limit: p.limit, before: p.before, action: p.action, actor: p.actor, q: p.q })}`),
  pii: () => request<PiiReport>("GET", "/policy/pii"),
  retention: () => request<RetentionQueue>("GET", "/policy/retention"),
  applyRetention: (dryRun, names) => request<RetentionApplyReport>("POST", `/policy/retention/apply${qs({ dry_run: dryRun })}`, names ? { names } : {}),
  modelCards: () => request<ModelCard[]>("GET", "/policy/model-card"),
  subjectAccess: (identifier) => request<SubjectAccessReport>("GET", `/policy/subject${qs({ q: identifier })}`),

  doctor: () => request<DoctorReport>("GET", "/doctor"),
  scan: (full) => request<ScanReport>("POST", `/scan${qs({ full })}`),
  command: (line) => request<CommandResult>("POST", "/command", { line }),
};
