/**
 * Mock transport. Deterministic (seeded PRNG), in-memory, and honest about what it is:
 * `transport: "mock"` is shown in the UI. Recall really does lexical + pseudo-semantic
 * candidate lists, RRF fusion and ring weighting (SPEC §7), so the Search screen behaves
 * like the real thing instead of returning canned rows.
 *
 * The shapes here follow `types.ts`, which follows the Rust wire structs — including the
 * server's quirks (empty `sources` on a hybrid result, `model_verified_at` always null,
 * flattened audit `detail`, the audit vocabulary), so the UI is not developed against a
 * fiction twice over. Where the server refuses something (a rename, a `names` subset for
 * retention, an unknown audit action), the mock refuses the same way.
 *
 * Citations here use FNV-1a, not blake3 — same shape (`r{ring}-{10 hex}`), different
 * bytes. They are only meaningful within a page load.
 */
import {
  type HubStatus,
  ApiError,
  type AuditAction,
  type AuditPage,
  type AuditParams,
  type AuditRow,
  type Citation,
  type CitationExpansion,
  type Conflict,
  type CyberbrainApi,
  type DoctorFinding,
  type DoctorReport,
  type EgressRegister,
  type ForgetReport,
  type Frontmatter,
  type Graph,
  type Hit,
  type IndexStats,
  type ModelCard,
  type NoteDetail,
  type NoteId,
  type NoteListParams,
  type NoteSummary,
  type NoteWriteRequest,
  type ObligationsView,
  type PiiFinding,
  type PiiHold,
  type PiiHoldResolution,
  type PiiReport,
  type PiiState,
  type RecallParams,
  type RecallResult,
  type RetentionApplyReport,
  type RetentionEntry,
  type RetentionQueue,
  type Ring,
  type ScanReport,
  type StatusReport,
  type UsageReport,
  type SubjectAccessReport,
} from "../types";
import { CLUSTERS, FILLER_SENTENCES, SEED_NOTES, type SeedNote } from "./data";

// ───────────────────────────────────────────────────────────────────── deterministic helpers

function mulberry32(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const rand = mulberry32(0xc7b3);
const pick = <T,>(xs: readonly T[]): T => xs[Math.floor(rand() * xs.length)] as T;

function fnv1a64(s: string): string {
  // 64-bit FNV-1a on UTF-16 code units, via two 32-bit halves. Enough for a mock.
  let h1 = 0x811c9dc5 ^ 0x2545f491;
  let h2 = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 0x01000193);
    h2 = Math.imul(h2 ^ (c * 31 + i), 0x01000193);
  }
  return ((h1 >>> 0).toString(16).padStart(8, "0") + (h2 >>> 0).toString(16).padStart(8, "0")).slice(0, 10);
}

/** 64 lowercase hex chars, the shape of a blake3 hash. */
const hex64 = (s: string) => (fnv1a64(s) + fnv1a64(s + "1") + fnv1a64(s + "2") + fnv1a64(s + "3") + fnv1a64(s + "4") + fnv1a64(s + "5") + fnv1a64(s + "6")).slice(0, 64);

const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
function ulid(tsMs: number): NoteId {
  let t = Math.floor(tsMs);
  let time = "";
  for (let i = 0; i < 10; i++) {
    time = CROCKFORD[t % 32] + time;
    t = Math.floor(t / 32);
  }
  let r = "";
  for (let i = 0; i < 16; i++) r += CROCKFORD[Math.floor(rand() * 32)];
  return time + r;
}

const NOW = Date.now();
const DAY = 86_400_000;
const iso = (msAgo: number) => new Date(NOW - msAgo).toISOString();
const daysAgo = (d: number) => iso(d * DAY + (d * 7919) % (DAY / 2)); // spread within the day

const PROFILE = "eu" as const;
const STORE_ROOT = "C:/Users/operator/project/.cyberbrain"; // forward slashes on every platform, also here
const INFERENCE_URL = "http://127.0.0.1:11434/v1";
const INFERENCE_SUMMARY = "http://127.0.0.1:11434"; // `Endpoint::summary()`: what the egress rows name
const MODEL_SOURCE = "https://huggingface.co/minishlab/potion-base-8M/resolve/main/model.safetensors";
const MODEL_NAME = "minishlab/potion-base-8M";
const MODEL_HASH = hex64("potion-base-8M");
const PROFILE_ID = "model2vec/potion-base-8M@d256/mean-l2";
const RESIDENT_CAP = 8192;

// ───────────────────────────────────────────────────────────────────── store model

interface MockBlock {
  citation: Citation;
  idx: number;
  text: string;
  token_count: number;
}

interface MockNote {
  front: Frontmatter;
  body: string;
  path: string;
  blocks: MockBlock[];
  outbound: string[];
}

const WIKILINK = /\[\[([^\]\n]+?)\]\]/g;
const approxTokens = (s: string) => Math.max(1, Math.round(s.split(/\s+/).filter(Boolean).length * 1.3));

/** `cyberbrain_core::validate_name`: kebab-case slug. Anything else can never be a note. */
const VALID_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
/** `cyberbrain_core::frontmatter::validate_retention`, the shape the UI can check. */
const VALID_RETENTION = /^P(?:\d+Y)?(?:\d+M)?(?:\d+W)?(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?(?:\d+S)?)?$/;

function splitBlocks(noteId: string, ring: Ring, body: string): MockBlock[] {
  // Heading and paragraph boundaries (SPEC §3.3). Fenced code stays whole.
  const parts: string[] = [];
  let cur: string[] = [];
  let inFence = false;
  const flush = () => {
    const t = cur.join("\n").trim();
    if (t) parts.push(t);
    cur = [];
  };
  for (const line of body.split("\n")) {
    if (line.startsWith("```")) inFence = !inFence;
    if (!inFence && (line.trim() === "" || /^#{1,6}\s/.test(line))) {
      flush();
      if (line.trim() !== "") cur.push(line);
      continue;
    }
    cur.push(line);
  }
  flush();
  // Merge a lone heading into the paragraph that follows it, as a real splitter would.
  const merged: string[] = [];
  for (const p of parts) {
    const prev = merged[merged.length - 1];
    if (prev !== undefined && /^#{1,6}\s[^\n]*$/.test(prev)) merged[merged.length - 1] = prev + "\n\n" + p;
    else merged.push(p);
  }
  return merged.map((text, idx) => ({
    citation: `r${ring}-${fnv1a64(noteId + " " + idx + " " + text)}`,
    idx,
    text,
    token_count: approxTokens(text),
  }));
}

const linkTargets = (body: string) => [...new Set([...body.matchAll(WIKILINK)].map((m) => (m[1] ?? "").trim()))];

/** Core's `Frontmatter` skips empty `tags`/`links` and an unset `retention`. */
function frontmatterOf(n: { id: NoteId; name: string; ring: Ring; kind: Frontmatter["kind"]; created: string; updated: string; tags: string[]; links: string[]; retention?: string | undefined; pii: PiiState }): Frontmatter {
  const f: Frontmatter = { id: n.id, name: n.name, ring: n.ring, kind: n.kind, created: n.created, updated: n.updated, pii: n.pii };
  if (n.tags.length) f.tags = n.tags;
  if (n.links.length) f.links = n.links;
  if (n.retention) f.retention = n.retention;
  return f;
}

function makeNote(seed: SeedNote, tsMs: number): MockNote {
  const id = ulid(tsMs);
  const outbound = linkTargets(seed.body);
  const front = frontmatterOf({ id, name: seed.name, ring: seed.ring, kind: seed.kind, created: daysAgo(seed.created), updated: daysAgo(seed.updated), tags: seed.tags, links: outbound, retention: seed.retention, pii: seed.pii ?? "none" });
  return { front, body: seed.body, path: `notes/r${seed.ring}/${seed.name}.md`, blocks: splitBlocks(id, seed.ring, seed.body), outbound };
}

function generateFiller(): SeedNote[] {
  const out: SeedNote[] = [];
  const seedNames = SEED_NOTES.map((n) => n.name);
  for (const c of CLUSTERS) {
    const clusterNames: string[] = [];
    for (let i = 1; i <= c.count; i++) {
      const name = c.prefix === "session" ? `session-2026-0${5 + Math.floor(i / 12)}-${String(1 + (i % 28)).padStart(2, "0")}-${i}` : `${c.prefix}-${String(i).padStart(3, "0")}-${pick(["note", "finding", "check", "trace", "measure", "review"])}`;
      clusterNames.push(name);
    }
    clusterNames.forEach((name, i) => {
      const links = new Set<string>();
      links.add(c.hub);
      if (rand() < 0.7 && i > 0) links.add(clusterNames[Math.floor(rand() * i)] as string);
      if (rand() < 0.35) links.add(pick(seedNames));
      if (rand() < 0.08) links.add(`${c.prefix}-todo-${Math.floor(rand() * 40)}`); // dangling: intent
      const created = c.kind === "session" ? 4 + i * 2 : Math.floor(rand() * 90);
      const title = name.replace(/-/g, " ");
      const body = `# ${title}\n\n${pick(FILLER_SENTENCES)} ${pick(FILLER_SENTENCES)}\n\nSee ${[...links].map((l) => `[[${l}]]`).join(", ")}.\n\n${pick(FILLER_SENTENCES)}`;
      const n: SeedNote = { name, ring: c.ring, kind: c.kind, tags: [...c.tags, pick(["s1", "s2", "ci", "local"])], created, updated: Math.max(0, created - Math.floor(rand() * 3)), body };
      if (c.kind === "session") n.retention = "P90D";
      // Imported material was never written through the tool, so nothing scanned it.
      if (c.ring === 4) n.pii = "unscanned";
      out.push(n);
    });
  }
  return out;
}

const STORE_INIT_MS = 61 * DAY;
const notes: MockNote[] = [];
{
  const all = [...SEED_NOTES, ...generateFiller()];
  all.forEach((s, i) => notes.push(makeNote(s, NOW - s.created * DAY + i)));
}
const byName = () => new Map(notes.map((n) => [n.front.name, n]));
const byId = () => new Map(notes.map((n) => [n.front.id, n]));

const err = (status: number, code: ApiError["body"]["code"], message: string, extra: Partial<ApiError["body"]> = {}) =>
  new ApiError(status, { code, message, exit_code: status === 403 || code === "pii-held" ? 3 : status >= 500 ? 2 : 1, ...extra });

function findNote(nameOrId: string): MockNote {
  const n = byId().get(nameOrId) ?? byName().get(nameOrId);
  if (!n) throw err(404, "not-found", `no note named ${nameOrId}`, { variant: "no-such-note" });
  return n;
}

function inboundOf(name: string): MockNote[] {
  return notes.filter((n) => n.outbound.includes(name));
}

function summary(n: MockNote): NoteSummary {
  const names = byName();
  const s: NoteSummary = {
    id: n.front.id,
    name: n.front.name,
    ring: n.front.ring,
    kind: n.front.kind,
    tags: n.front.tags ?? [],
    updated: n.front.updated,
    created: n.front.created,
    pii: n.front.pii,
    blocks: n.blocks.length,
    bytes: new TextEncoder().encode(n.body).length + 180,
    links_out: n.outbound.length,
    links_in: inboundOf(n.front.name).length,
    dangling: n.outbound.filter((t) => !names.has(t)).length,
  };
  if (n.front.retention) s.retention = n.front.retention;
  return s;
}

function detail(n: MockNote): NoteDetail {
  const names = byName();
  return {
    front: n.front,
    body: n.body,
    path: n.path,
    outbound: n.outbound.map((target) => {
      const t = names.get(target);
      return { target, resolved: t ? { id: t.front.id, ring: t.front.ring } : null };
    }),
    inbound: inboundOf(n.front.name).map((m) => ({ from: { id: m.front.id, name: m.front.name, ring: m.front.ring } })),
    blocks: n.blocks.map((b) => ({ citation: b.citation, idx: b.idx, token_count: b.token_count, preview: b.text.split("\n")[0]?.replace(/^#+\s*/, "").slice(0, 80) ?? "" })),
  };
}

// ───────────────────────────────────────────────────────────────────── audit log

/** A row as `cyberbrain_policy::AuditEvent` would carry it, before flattening. */
interface Event {
  ts: string;
  actor: string;
  action: AuditAction;
  subject: string;
  detail: Record<string, unknown>;
}

const events: Event[] = [];
function log(msAgo: number, actor: string, action: AuditAction, subject: string, detail: Record<string, unknown> = {}) {
  events.push({ ts: iso(msAgo), actor, action, subject, detail });
}

/** `serve::policy::flat_detail`: scalars stay, anything nested becomes its JSON text. */
function flatDetail(d: Record<string, unknown>): AuditRow["detail"] {
  const out: AuditRow["detail"] = {};
  for (const [k, v] of Object.entries(d)) {
    if (k === "_chain") continue;
    out[k] = v === null || typeof v === "string" || typeof v === "number" || typeof v === "boolean" ? v : JSON.stringify(v);
  }
  return out;
}

const OPERATOR = "operator"; // `cyberbrain serve` runs as `Actor::Operator`

function inferenceCall(msAgo: number, actor: string, task: string, model: string, tokensIn: number, tokensOut: number, ms: number) {
  log(msAgo, actor, "egress.permitted", `local-inference:${INFERENCE_SUMMARY}`, { purpose: "local-inference", destination: INFERENCE_SUMMARY, locality: "loopback", profile: PROFILE });
  log(msAgo - 1, actor, "inference.call", `inference:${INFERENCE_URL}`, {
    endpoint: INFERENCE_URL,
    model,
    outcome: "ok",
    prompt_tokens: tokensIn,
    completion_tokens: tokensOut,
    total_tokens: tokensIn + tokensOut,
    purpose: "local-inference",
    call: "chat.completions",
    task,
    resolved: "127.0.0.1:11434",
    public_waived: false,
    elapsed_ms: ms,
    outcome_detail: { kind: "ok" },
  });
  log(msAgo - 2, actor, "egress.completed", `local-inference:${INFERENCE_SUMMARY}`, { ticket: ulid(NOW - msAgo), purpose: "local-inference", status: 200, bytes_out: tokensIn * 4, bytes_in: tokensOut * 4 + 220, elapsed_ms: ms });
}

{
  log(STORE_INIT_MS, OPERATOR, "store.init", `store:${STORE_ROOT}`, { profile: PROFILE });
  log(STORE_INIT_MS - 60_000, OPERATOR, "consent.model-download", "model-download", { consent: true, model_source: MODEL_SOURCE });
  log(STORE_INIT_MS - 3 * 60_000, OPERATOR, "egress.permitted", `model-download:${MODEL_SOURCE}`, { purpose: "model-download", destination: MODEL_SOURCE, locality: "public", profile: PROFILE });
  log(STORE_INIT_MS - 3 * 60_000 + 4_100, OPERATOR, "egress.completed", `model-download:${MODEL_SOURCE}`, { ticket: ulid(NOW - STORE_INIT_MS), purpose: "model-download", status: 200, bytes_out: 412, bytes_in: 31_457_280, elapsed_ms: 4_100 });
  log(STORE_INIT_MS - 4 * 60_000, "cli", "index.embedding-profile-changed", `embedding:${PROFILE_ID}`, { previous: null, current: { id: PROFILE_ID, dim: 256, model_hash: MODEL_HASH }, changed: true, vectors_wiped: 0 });
  log(STORE_INIT_MS - 5 * 60_000, "cli", "index.cleared", "index", { erased: { notes: 0, blocks: 0, fts_rows: 0, vectors: 0, links_out: 0, links_in_unresolved: 0 }, dry_run: false });
  // Writes, one per note, at the note's creation time. Imported notes were never written
  // through the tool, so they have no row: the log knows nothing about them.
  for (const n of notes) {
    if (n.front.pii === "unscanned") continue;
    const ago = NOW - Date.parse(n.front.created);
    log(ago, n.front.ring === 3 ? "hook:stop" : "agent:claude-code", "note.write", `note:${n.front.name}`, { name: n.front.name, ring: n.front.ring, kind: n.front.kind, bytes: new TextEncoder().encode(n.body).length + 180, pii: n.front.pii, scanned: true });
  }
  // Inference calls over the last 30 days.
  const models = ["qwen3:8b", "qwen3:8b", "qwen3:8b", "gemma3:12b"];
  for (let i = 0; i < 140; i++) {
    inferenceCall(rand() * 30 * DAY, pick(["agent:claude-code", "mcp", "cli"]), pick(["contradiction-check", "contradiction-check", "propose-ring", "summarise-session"]), pick(models), 400 + Math.floor(rand() * 1800), 1 + Math.floor(rand() * 90), 120 + Math.floor(rand() * 900));
  }
  log(19 * DAY, "cli", "policy.refusal", "local-inference:https://api.example-cloud.com:443", { purpose: "local-inference", reason: "https://api.example-cloud.com resolves to a public address and allow_public_endpoint is false", profile: PROFILE });
  {
    const held = notes.find((n) => n.front.name === "contact-notes-vendor-call");
    if (held) {
      const ago = NOW - Date.parse(held.front.created);
      log(ago + 20_000, OPERATOR, "note.write.held", `note:${held.front.name}`, { profile: PROFILE, findings: [{ kind: "email", start: 118, end: 141, confidence: "high" }, { kind: "phone", start: 156, end: 172, confidence: "medium" }] });
      log(ago + 5_000, OPERATOR, "note.write.resolved", `note:${held.front.name}`, { choice: "mark-reviewed", pii: "reviewed", redacted: 0, remaining: 2 });
    }
  }
  {
    const gone = ulid(NOW - 100 * DAY);
    log(11 * DAY, "retention", "retention.expired", `note:${gone}`, { name: "session-2026-05-11", ring: 3, retention: "P90D", created: iso(101 * DAY), expired_at: iso(11 * DAY), evaluated_at: iso(11 * DAY) });
    log(11 * DAY - 1_000, "retention", "note.erase.requested", `note:${gone}`, { name: "session-2026-05-11", ring: 3, reason: "retention-expired", basis: "GDPR Art. 5(1)(e) storage limitation", profile: PROFILE, dry_run: false });
    log(11 * DAY - 2_000, "retention", "note.erase.completed", `note:${gone}`, { name: "session-2026-05-11", reason: "retention-expired", dry_run: false, file_removed: true, blocks: 3, fts_rows: 3, vectors: 3, links_out: 2, links_in_unresolved: 0, derivatives: 0 });
  }
  log(2 * DAY, OPERATOR, "subject.access", `identifier:${fnv1a64("j.doe@example.org")}`, { profile: PROFILE, note_hits: 1, audit_hits: 0, trimmed: 0, response_deadline: "one month from receipt (GDPR Art. 12(3)), extendable by two months" });
  log(1 * DAY, "cli", "audit.export", "audit", { rows: events.length, format: "jsonl" });
  events.sort((a, b) => Date.parse(a.ts) - Date.parse(b.ts));
}

/** `serve::policy::action_matches`: exact name, family prefix, or the old vocabulary. */
function actionMatches(filter: string, e: Event): boolean {
  const a = e.action;
  switch (filter) {
    case "write":
      return a.startsWith("note.write");
    case "erase":
      return a.startsWith("note.erase");
    case "scan":
      return a.startsWith("index.") || a === "store.init";
    case "model-download":
      return a.startsWith("egress.") && e.subject.startsWith("model-download:");
    case "inference":
      return a === "inference.call";
    case "policy-refusal":
    case "egress-refused":
      return a === "policy.refusal";
    case "egress-attempt":
      return a.startsWith("egress.");
    case "retention-apply":
      return a === "retention.expired";
    case "hold-resolved":
      return a === "note.write.resolved" || a === "note.write.discarded";
    case "subject-access":
      return a === "subject.access";
    default:
      return a === filter || a.startsWith(`${filter}.`);
  }
}

// ───────────────────────────────────────────────────────────────────── recall

const STOP = new Set("a an the of to in on for and or is are was be by with as at from it this that not no into over".split(" "));
const tokenize = (s: string) => s.toLowerCase().replace(/[`*_#|\\[\]()>]/g, " ").split(/[^a-z0-9.\-]+/).filter((t) => t.length > 1 && !STOP.has(t));
const trigrams = (s: string) => {
  const t = new Set<string>();
  const x = ` ${s.toLowerCase().replace(/[^a-z0-9]+/g, " ")} `;
  for (let i = 0; i + 3 <= x.length; i++) t.add(x.slice(i, i + 3));
  return t;
};

/** `Ring::weight`: deliberately close to 1 (see core). */
const RING_W: Record<Ring, number> = { 0: 1.15, 1: 1.1, 2: 1.0, 3: 0.92, 4: 0.8 };

interface Candidate {
  note: MockNote;
  block: MockBlock;
}

function recallImpl(p: RecallParams): RecallResult {
  const q = p.q.trim();
  if (!q) throw err(400, "bad-request", "`q` must not be empty");
  if (p.n !== undefined && (p.n < 1 || p.n > 100)) throw err(400, "bad-request", `\`n\` must be between 1 and 100, not ${p.n}`);
  const n = p.n ?? 8;
  const t0 = performance.now();
  const qTok = tokenize(q);
  const qTri = trigrams(q);
  const pool: Candidate[] = [];
  for (const note of notes) {
    if (p.ring !== undefined && note.front.ring !== p.ring) continue;
    for (const block of note.blocks) pool.push({ note, block });
  }
  // Lexical: a BM25-shaped score. Document frequency computed over the pool.
  const df = new Map<string, number>();
  const docs = pool.map((c) => {
    const toks = tokenize(c.block.text + " " + c.note.front.name + " " + (c.note.front.tags ?? []).join(" "));
    for (const t of new Set(toks)) df.set(t, (df.get(t) ?? 0) + 1);
    return toks;
  });
  const avgLen = docs.reduce((a, d) => a + d.length, 0) / Math.max(1, docs.length);
  const lex = pool
    .map((c, i) => {
      const toks = docs[i] ?? [];
      let s = 0;
      for (const t of qTok) {
        const tf = toks.filter((x) => x === t || x.startsWith(t)).length;
        if (!tf) continue;
        const idf = Math.log(1 + (pool.length - (df.get(t) ?? 0) + 0.5) / ((df.get(t) ?? 0) + 0.5));
        s += idf * ((tf * 2.2) / (tf + 1.2 * (0.25 + 0.75 * (toks.length / avgLen))));
      }
      return { c, s };
    })
    .filter((x) => x.s > 0)
    .sort((a, b) => b.s - a.s)
    .slice(0, 50);
  // "Semantic": trigram Jaccard stands in for cosine. Different candidates than lexical, which is the point.
  const sem = pool
    .map((c) => {
      const tri = trigrams(c.block.text);
      let inter = 0;
      for (const g of qTri) if (tri.has(g)) inter++;
      const s = inter / (qTri.size + tri.size - inter);
      return { c, s };
    })
    .filter((x) => x.s > 0.02)
    .sort((a, b) => b.s - a.s)
    .slice(0, 50);
  // RRF.
  const fused = new Map<string, { c: Candidate; score: number }>();
  const add = (list: Array<{ c: Candidate }>) =>
    list.forEach(({ c }, rank) => {
      const e = fused.get(c.block.citation) ?? { c, score: 0 };
      e.score += 1 / (60 + rank + 1);
      fused.set(c.block.citation, e);
    });
  add(lex);
  add(sem);
  const hits: Hit[] = [...fused.values()]
    .map((e) => ({ ...e, score: e.score * RING_W[e.c.note.front.ring] }))
    .sort((a, b) => b.score - a.score)
    .slice(0, n)
    .map((e) => ({
      citation: e.c.block.citation,
      note_id: e.c.note.front.id,
      note_name: e.c.note.front.name,
      ring: e.c.note.front.ring,
      score: Number(e.score.toFixed(5)),
      text: e.c.block.text,
      block_idx: e.c.block.idx,
      // The index does not report per-hit candidate lists; the server sends [] for a
      // hybrid result and says so in the caveats. Same here, even though this mock knows.
      sources: [],
    }));
  // Contradiction check: the one canned pair the seed data contains, only when both appear.
  const conflicts: Conflict[] = [];
  const caveats: string[] = [];
  const winner = hits.find((h) => h.note_name === "pg18-moves-pgdata");
  const loser = hits.find((h) => h.note_name === "imported-postgres-data-dir-claim");
  if (winner && loser) {
    conflicts.push({ winner: winner.citation, loser: loser.citation, reason: "r4 claims the data directory never moves; r2 records that PostgreSQL 18 moved it. Lower ring wins." });
    inferenceCall(0, OPERATOR, "contradiction-check", "qwen3:8b", 812, 3, 340);
  } else if (hits.length >= 2) {
    caveats.push("contradiction check ran on 1 cross-ring pair and found none (qwen3:8b via ollama, 290 ms)");
  } else {
    caveats.push("contradiction check skipped: fewer than two hits");
  }
  if (hits.length) caveats.push("per-hit candidate sources are not reported by the index; `sources` is empty for a hybrid result");
  return {
    hits,
    conflicts,
    caveats,
    mode: "hybrid",
    elapsed_ms: Number((performance.now() - t0).toFixed(1)),
    params: { q, n, ring: p.ring ?? null, k_lex: 50, k_sem: 50 },
  };
}

// ───────────────────────────────────────────────────────────────────── PII scan (§12.4)

const PII_PATTERNS: Array<{ kind: PiiFinding["kind"]; re: RegExp; mask: (m: string) => string }> = [
  { kind: "email", re: /[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}/gi, mask: (m) => m[0] + "***@" + m.split("@")[1] },
  { kind: "ip", re: /\b(?!127\.0\.0\.1)(?:\d{1,3}\.){3}\d{1,3}\b/g, mask: (m) => m.replace(/\d+\.\d+$/, "*.*") },
  { kind: "api-key", re: /\b(?:sk|pk|ghp|xox[bp]|AKIA)[-_A-Za-z0-9]{16,}\b/g, mask: (m) => m.slice(0, 6) + "…" },
  { kind: "iban", re: /\b[A-Z]{2}\d{2}(?:\s?[A-Z0-9]{4}){3,7}\b/g, mask: (m) => m.slice(0, 4) + " **** ****" },
  { kind: "phone", re: /(?:\+|00)\d{1,3}[\s\d\-]{7,}\d/g, mask: (m) => m.slice(0, 4) + "…" + m.slice(-2) },
];

function scanPii(body: string): PiiFinding[] {
  const out: PiiFinding[] = [];
  const lines = body.split("\n");
  lines.forEach((line, li) => {
    for (const p of PII_PATTERNS) {
      for (const m of line.matchAll(p.re)) out.push({ kind: p.kind, excerpt: p.mask(m[0]), line: li + 1, col: (m.index ?? 0) + 1 });
    }
  });
  return out;
}

const holds = new Map<string, { hold: PiiHold; nameOrId: string; req: NoteWriteRequest; findings: PiiFinding[] }>();

function applyWrite(n: MockNote, req: NoteWriteRequest, pii: PiiState) {
  n.body = req.body;
  n.outbound = linkTargets(req.body);
  const tags = req.front?.tags ?? n.front.tags ?? [];
  const kind = req.front?.kind ?? n.front.kind;
  let retention = n.front.retention;
  if (req.front?.retention !== undefined) retention = req.front.retention || undefined;
  n.front = frontmatterOf({ id: n.front.id, name: n.front.name, ring: n.front.ring, kind, created: n.front.created, updated: new Date().toISOString(), tags, links: n.outbound, retention, pii });
  n.blocks = splitBlocks(n.front.id, n.front.ring, n.body);
  log(0, OPERATOR, "note.write", `note:${n.front.name}`, { name: n.front.name, ring: n.front.ring, kind: n.front.kind, bytes: new TextEncoder().encode(n.body).length + 180, pii, scanned: true });
}

function forgetImpl(n: MockNote, dryRun: boolean, reason: "operator-forget" | "retention-expired"): ForgetReport {
  const report: ForgetReport = {
    dry_run: dryRun,
    note: { id: n.front.id, name: n.front.name, path: n.path },
    removed: { file: true, blocks: n.blocks.length, vectors: n.blocks.length, fts_rows: n.blocks.length, links_in: inboundOf(n.front.name).length, links_out: n.outbound.length, derivatives: 0 },
    notes: [],
  };
  const subject = `note:${n.front.id}`;
  const actor = reason === "retention-expired" ? "retention" : OPERATOR;
  if (dryRun) {
    report.notes.push(`audit row a real run would append: note.erase.requested ${subject}`, `audit row a real run would append: note.erase.completed ${subject}`);
  } else {
    log(0, actor, "note.erase.requested", subject, { name: n.front.name, ring: n.front.ring, reason, basis: "GDPR Art. 17 (right to erasure); recipients informed per Art. 19", profile: PROFILE, dry_run: false });
    notes.splice(notes.indexOf(n), 1);
    log(0, actor, "note.erase.completed", subject, { name: n.front.name, reason, dry_run: false, file_removed: true, ...report.removed });
  }
  return report;
}

// ───────────────────────────────────────────────────────────────────── misc helpers

const latency = <T,>(v: T, ms = 40 + rand() * 80): Promise<T> => new Promise((r) => setTimeout(() => r(structuredClone(v)), ms));
const bytesOf = (s: string) => new TextEncoder().encode(s).length;

/** `cyberbrain_policy::retention::queue`: due first, then pending by expiry, then invalid. */
function retentionEntries(): RetentionEntry[] {
  const entries = notes
    .filter((n) => n.front.retention)
    .map((n): RetentionEntry => {
      const retention = n.front.retention ?? "";
      const note = { id: n.front.id, name: n.front.name, ring: n.front.ring };
      const m = /^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)W)?(?:(\d+)D)?$/.exec(retention);
      if (!m || !VALID_RETENTION.test(retention)) {
        return { note, retention, expires_at: n.front.created, due: false, invalid: retention.startsWith("P") ? "a unit letter must follow a number" : "must start with `P`" };
      }
      const days = Number(m[1] ?? 0) * 365 + Number(m[2] ?? 0) * 30 + Number(m[3] ?? 0) * 7 + Number(m[4] ?? 0);
      const expires = Date.parse(n.front.created) + days * DAY;
      return { note, retention, expires_at: new Date(expires).toISOString(), due: expires <= NOW };
    });
  const rank = (e: RetentionEntry) => (e.invalid !== undefined ? 2 : e.due ? 0 : 1);
  return entries.sort((a, b) => rank(a) - rank(b) || Date.parse(a.expires_at) - Date.parse(b.expires_at));
}

function indexStats(): IndexStats {
  const names = byName();
  const blocks = notes.reduce((a, n) => a + n.blocks.length, 0);
  return {
    schema_version: 3,
    generation: 41,
    notes: notes.length,
    blocks,
    fts_rows: blocks,
    vectors: blocks,
    links: notes.reduce((a, n) => a + n.outbound.length, 0),
    dangling_links: notes.reduce((a, n) => a + n.outbound.filter((t) => !names.has(t)).length, 0),
    embedding: { id: PROFILE_ID, dim: 256, model_hash: MODEL_HASH },
  };
}

let lastScan: string | null = null;
let lastFullScan: string | null = null;

// ───────────────────────────────────────────────────────────────────── the client

export const mockClient: CyberbrainApi = {
  transport: "mock",

  // Fabricated like everything else here, and shaped so the layout is exercised: one op
  // with cache reports, one without.
  async usage(days = 30): Promise<UsageReport> {
    return {
      retrieval: {
        since: new Date(NOW - 36 * 3600_000).toISOString(),
        rows: 41,
        recall: { ops: 33, returned: 62_140, full: 486_920, hits: 248 },
        find: { ops: 8, returned: 214, full: 4_120, hits: 19 },
        unreadable_rows: 0,
      },
      inference: {
        tasks: {
          "contradiction-check": { calls: 21, failed: 2, prompt_tokens: 42_180, cached_prompt_tokens: 9_640, completion_tokens: 1_902, elapsed_ms: 411_000, calls_without_counts: 0, calls_without_cache_report: 3 },
          "session-summary": { calls: 2, failed: 0, prompt_tokens: 8_940, cached_prompt_tokens: 0, completion_tokens: 1_180, elapsed_ms: 39_000, calls_without_counts: 0, calls_without_cache_report: 2 },
        },
        first: new Date(NOW - 30 * 3600_000).toISOString(),
        last: new Date(NOW - 900_000).toISOString(),
      },
      load: {
        calls: 23,
        wall_ms: 450_000,
        machine_cores_avg: 6.2,
        endpoint_cores_avg: 5.9,
        last: { at: new Date(NOW - 900_000).toISOString(), task: "contradiction-check", wall_ms: 38_500, machine_cores: 6.1, machine_mem_delta_mb: 84, endpoint_cores: 5.9, endpoint_mem_bytes: 9_480_000_000, endpoint_mem_peak_bytes: 10_200_000_000 },
        calls_without_attribution: 4,
        cores_total: 12,
      },
      days: Array.from({ length: days }, (_, i) => {
        const d = new Date(NOW - (days - 1 - i) * 86_400_000).toISOString().slice(0, 10);
        const busy = i > days - 6 || i % 7 === 3;
        const r = busy ? 4 + ((i * 7) % 9) : 0;
        return {
          date: d,
          recall: { ops: r, returned: r * 1_900, full: r * 14_800, hits: r * 8 },
          find: { ops: busy ? 2 : 0, returned: busy ? 48 : 0, full: busy ? 910 : 0, hits: busy ? 3 : 0 },
          calls: r,
          prompt_tokens: r * 2_100,
          cached_prompt_tokens: r * 480,
          completion_tokens: r * 90,
          wall_ms: r * 41_000,
          endpoint_cores: busy ? 5.6 + ((i % 3) * 0.2) : null,
          machine_cores: busy ? 5.9 + ((i % 3) * 0.2) : null,
        };
      }),
      loaded_models: [{ name: "qwen2.5:7b-instruct", size: 5_062_566_870, size_vram: 0, context_length: 4096, parameter_size: "7.6B", quantization_level: "Q4_K_M", expires_at: new Date(NOW + 240_000).toISOString() }],
    };
  },

  async hubStatus(): Promise<HubStatus> {
    // The mock shows a machine that reports to a hub, because the interesting half of this
    // line is the one with something in it — an empty state that is always empty teaches
    // nobody what the screen looks like when it matters.
    return {
      enrolled: true,
      hub: "https://hub.example.internal:7788",
      device: "dev_01M1Y70BYZM68NWYVE03ZF09YM",
      last_delivery: new Date(NOW - 11 * 60_000).toISOString(),
    };
  },

  async status(): Promise<StatusReport> {
    const rings = ([0, 1, 2, 3, 4] as Ring[]).map((ring) => {
      const ns = notes.filter((n) => n.front.ring === ring);
      return { ring, notes: ns.length, blocks: ns.reduce((a, n) => a + n.blocks.length, 0), tokens: ns.reduce((a, n) => a + n.blocks.reduce((b, x) => b + x.token_count, 0), 0), bytes: ns.reduce((a, n) => a + bytesOf(n.body) + 180, 0) };
    });
    const ix = indexStats();
    const notesBytes = rings.reduce((a, r) => a + r.bytes, 0);
    const dbBytes = ix.blocks * 256 * 4 + ix.blocks * 900 + 65536;
    const lastInference = [...events].reverse().find((r) => r.action === "inference.call");
    const caveats: string[] = [];
    if (lastScan === null) caveats.push("last_scan is known only for scans run through this server since it started; CLI scans leave no timestamp");
    caveats.push("model_verified_at: the artefact hash is verified on every load but no timestamp of that load is kept");
    return latency({
      version: "0.1.0-dev (mock)",
      store: { path: STORE_ROOT, bytes: notesBytes + dbBytes, notes_bytes: notesBytes, db_bytes: dbBytes, models_bytes: 31_457_280, notes: notes.length, blocks: ix.blocks, vectors: ix.vectors, rings, resident_cap: { tokens: RESIDENT_CAP, used: (rings[0]?.tokens ?? 0) + (rings[1]?.tokens ?? 0) } },
      index: { schema_version: ix.schema_version, last_scan: lastScan, last_full_scan: lastFullScan ?? iso(STORE_INIT_MS - 5 * 60_000), stale_notes: 1, orphan_vectors: 0, dangling_links: ix.dangling_links, fts_ok: true },
      embedding: { profile_id: PROFILE_ID, model: MODEL_NAME, dim: 256, pooling: "mean, L2-normalised (SPEC §6.2)", backend: "static", matches_index: true, model_hash: MODEL_HASH, model_verified_at: null, loaded: true },
      inference: { configured: true, base_url: INFERENCE_URL, endpoint_class: "loopback", allow_public_endpoint: false, model: "qwen3:8b", last_backend: "ollama", last_backend_evidence: 'response header "x-ollama-version: 0.12.3"; model list shape', last_call: lastInference?.ts ?? null, reachable: true, reachable_checked_at: new Date().toISOString() },
      policy: { profile: PROFILE, pii_scan: true, audit_rows: events.length },
      caveats,
    });
  },

  async recall(p) {
    return latency(recallImpl(p), 25 + rand() * 40);
  },

  async expand(citation): Promise<CitationExpansion> {
    for (const n of notes) {
      const b = n.blocks.find((x) => x.citation === citation);
      if (b) return latency({ citation, ring: n.front.ring, block_idx: b.idx, block_text: b.text, token_count: b.token_count, note: detail(n) });
    }
    throw err(404, "not-found", `citation ${citation} does not resolve; the block text may have changed since it was issued`, { variant: "bad-citation" });
  },

  async listNotes(p: NoteListParams = {}) {
    let xs = notes.map(summary);
    if (p.ring !== undefined) xs = xs.filter((n) => n.ring === p.ring);
    if (p.kind) xs = xs.filter((n) => n.kind === p.kind);
    if (p.q) {
      const q = p.q.toLowerCase();
      xs = xs.filter((n) => n.name.toLowerCase().includes(q) || n.tags.some((t) => t.toLowerCase().includes(q)));
    }
    const sort = p.sort ?? "updated";
    xs.sort((a, b) => (sort === "name" ? a.name.localeCompare(b.name) : sort === "ring" ? a.ring - b.ring || a.name.localeCompare(b.name) : Date.parse(b.updated) - Date.parse(a.updated) || a.name.localeCompare(b.name)));
    return latency(xs);
  },

  async getNote(nameOrId) {
    return latency(detail(findNote(nameOrId)));
  },

  async writeNote(nameOrId, req) {
    const n = findNote(nameOrId);
    if (req.front) {
      for (const k of Object.keys(req.front)) {
        if (["id", "created", "links", "updated", "pii"].includes(k)) throw err(400, "bad-frontmatter", `\`${k}\` is server-owned and cannot be set through a write`);
        if (!["name", "ring", "kind", "tags", "retention"].includes(k)) throw err(400, "bad-frontmatter", `\`front.${k}\` is not a frontmatter field a write may set`);
      }
      if (req.front.name !== undefined && req.front.name !== n.front.name) {
        throw err(400, "bad-request", `renaming \`${n.front.name}\` to \`${req.front.name}\` is not supported through the API yet: a write under a new name would create a second note with a new id rather than move this one`);
      }
      if (req.front.ring !== undefined && req.front.ring !== n.front.ring) {
        throw err(400, "bad-frontmatter", `name \`${n.front.name}\` already exists in ring r${n.front.ring} at ${n.path}; remove it first to move the note`, { variant: "frontmatter" });
      }
      if (req.front.retention && !VALID_RETENTION.test(req.front.retention)) {
        throw err(400, "bad-frontmatter", `retention \`${req.front.retention}\`: must start with \`P\``, { variant: "frontmatter" });
      }
    }
    if (req.expected_updated !== n.front.updated) {
      throw err(409, "write-conflict", `note \`${n.front.name}\` changed at ${n.front.updated} since you loaded it; reload and reapply your edit`, { current_updated: n.front.updated });
    }
    if (n.front.ring <= 1) {
      const used = notes.filter((x) => x.front.ring <= 1 && x !== n).reduce((a, x) => a + x.blocks.reduce((b, y) => b + y.token_count, 0), 0);
      const wouldBe = used + approxTokens(req.body);
      if (wouldBe > RESIDENT_CAP) throw err(400, "ring-cap-exceeded", `rings 0+1 would hold ${wouldBe} tokens, cap is ${RESIDENT_CAP}`, { cap: { tokens: RESIDENT_CAP, used, would_be: wouldBe }, variant: "ring-cap-exceeded" });
    }
    const findings = scanPii(req.body);
    if (findings.length) {
      const hold: PiiHold = { hold_id: ulid(Date.now()), note: n.front.name, findings, expires_at: iso(-15 * 60_000) };
      holds.set(hold.hold_id, { hold, nameOrId, req, findings });
      log(0, OPERATOR, "note.write.held", `note:${n.front.name}`, { profile: PROFILE, findings: findings.map((f) => ({ kind: f.kind, start: f.col, end: f.col + f.excerpt.length, confidence: "high" })) });
      throw err(409, "pii-held", `write of \`${n.front.name}\` held: ${findings.length} possible personal data item${findings.length === 1 ? "" : "s"} found (profile ${PROFILE}); redact, mark reviewed, proceed flagged, or discard`, { hold });
    }
    applyWrite(n, req, "none");
    return latency(detail(n));
  },

  async resolveHold(holdId, res: PiiHoldResolution) {
    if (!["redact", "mark-reviewed", "proceed", "discard"].includes(res.action)) throw err(400, "bad-request", `action ${JSON.stringify(res.action)} is not one of redact, mark-reviewed, proceed, discard`);
    const h = holds.get(holdId);
    if (!h) throw err(404, "not-found", `hold ${holdId} is unknown or has expired; resubmit the write`);
    holds.delete(holdId);
    const n = findNote(h.nameOrId);
    if (res.action === "discard") {
      log(0, OPERATOR, "note.write.discarded", `note:${n.front.name}`, { findings: h.findings.length, dry_run: false, hold: holdId });
      return latency(null);
    }
    let body = h.req.body;
    if (res.action === "redact") {
      for (const p of PII_PATTERNS) body = body.replace(p.re, `[redacted:${p.kind}]`);
    }
    const pii: PiiState = res.action === "redact" ? "none" : res.action === "mark-reviewed" ? "reviewed" : "flagged";
    log(0, OPERATOR, "note.write.resolved", `note:${n.front.name}`, { choice: res.action === "proceed" ? "proceed-flagged" : res.action, pii, redacted: res.action === "redact" ? h.findings.length : 0, remaining: res.action === "redact" ? 0 : h.findings.length });
    applyWrite(n, { ...h.req, body }, pii);
    return latency(detail(n));
  },

  async forget(nameOrId, dryRun) {
    return latency(forgetImpl(findNote(nameOrId), dryRun, "operator-forget"));
  },

  async graph(): Promise<Graph> {
    const names = byName();
    const g: Graph = { nodes: [], edges: [], dangling: [] };
    for (const n of notes) {
      g.nodes.push({ id: n.front.id, name: n.front.name, ring: n.front.ring, kind: n.front.kind, links_in: inboundOf(n.front.name).length, links_out: n.outbound.length });
      for (const t of n.outbound) {
        const target = names.get(t);
        if (target) g.edges.push({ from: n.front.id, to: target.front.id });
        else g.dangling.push({ from: n.front.id, to_name: t });
      }
    }
    return latency(g, 120);
  },

  // The three lines a reader would check first, in the two confidence levels that make the
  // point. Fabricated like the rest of this file; the real catalogue is longer.
  async obligations(): Promise<ObligationsView> {
    return latency({
      profile: PROFILE,
      law: "GDPR, Regulation (EU) 2016/679",
      obligations: [
        {
          topic: "erasure",
          summary: "Data subjects may demand erasure; the controller erases without undue delay and informs recipients.",
          basis: "GDPR Art. 17, Art. 19",
          confidence: "high",
          note: "`cyberbrain forget` is the mechanism; the audit row is the evidence.",
        },
        {
          topic: "access",
          summary: "Data subjects may obtain a copy of their data. Respond within one month.",
          basis: "GDPR Art. 15, Art. 12(3)",
          confidence: "high",
          note: "",
        },
        {
          topic: "ai-regulation",
          summary: "The EU AI Act applies in the EU. Cyberbrain prints a model card so a deployer inside a regulated workflow has identity, source, licence and hash on paper.",
          basis: "Regulation (EU) 2024/1689",
          confidence: "low",
          note: "Risk class depends on the deployer's use; this tool cannot determine it.",
        },
      ],
    });
  },

  async egress(): Promise<EgressRegister> {
    const path = (purpose: "model-download" | "local-inference") => {
      const prefix = `${purpose}:`;
      const permitted = events.filter((e) => e.action === "egress.permitted" && e.subject.startsWith(prefix));
      const bytesOut = events.filter((e) => e.action === "egress.completed" && e.subject.startsWith(prefix)).reduce((a, e) => a + Number(e.detail.bytes_out ?? 0), 0);
      return { permitted, bytesOut, last: permitted[permitted.length - 1]?.ts ?? null };
    };
    const dl = path("model-download");
    const inf = path("local-inference");
    const dlState = "artefact present and hash-verified; nothing to download";
    const infState = `enabled: http://127.0.0.1:11434 is loopback; model qwen3:8b configured`;
    return latency({
      profile: PROFILE,
      since: events[0]?.ts ?? new Date().toISOString(),
      register_hash: "b3:" + fnv1a64("model-download|local-inference"),
      refused_total: events.filter((r) => r.action === "policy.refusal").length,
      paths: [
        {
          purpose: "model-download",
          description: "downloads a model artefact from the configured source, once, after you agree",
          destination: MODEL_SOURCE,
          destination_class: "public",
          data: "HTTP GET for the artefact; request carries no note content, no identifier, no telemetry",
          permitted_by: ["eu", "ch", "off"],
          enabled: false,
          disabled_reason: dlState,
          last_used: dl.last,
          uses_total: dl.permitted.length,
          bytes_out_total: dl.bytesOut,
          state: dlState,
          carries_note_content: false,
        },
        {
          purpose: "local-inference",
          description: "sends note text to the configured inference endpoint on your own network",
          destination: INFERENCE_URL,
          destination_class: "loopback",
          data: "prompt with the retrieved block texts for contradiction checks, ring proposals and session summaries",
          permitted_by: ["eu", "ch", "off"],
          enabled: true,
          disabled_reason: null,
          last_used: inf.last,
          uses_total: inf.permitted.length,
          bytes_out_total: inf.bytesOut,
          state: infState,
          carries_note_content: true,
        },
      ],
    });
  },

  async audit(p: AuditParams = {}): Promise<AuditPage> {
    const limit = Math.min(Math.max(p.limit ?? 100, 1), 1000);
    const action = p.action?.trim() || undefined;
    const actor = p.actor?.trim() || undefined;
    const q = p.q?.trim().toLowerCase() || undefined;
    let rows: AuditRow[] = events
      .map((e, i): AuditRow => ({ seq: i + 1, ts: e.ts, actor: e.actor, action: e.action, subject: e.subject, detail: flatDetail(e.detail) }))
      .reverse()
      .filter((_r, i) => action === undefined || actionMatches(action, events[events.length - 1 - i] as Event))
      .filter((r) => actor === undefined || r.actor.startsWith(actor))
      .filter((r) => q === undefined || `${r.actor} ${r.action} ${r.subject} ${JSON.stringify(r.detail)}`.toLowerCase().includes(q));
    if (rows.length === 0 && action !== undefined && events.length && q === undefined && actor === undefined) {
      const present = [...new Set(events.map((e) => e.action))].sort();
      throw err(400, "bad-request", `no audit action named ${JSON.stringify(action)}; the log contains: ${present.join(", ")}. Refused rather than answered with an empty table: a missing name and a thing that never happened are different answers`);
    }
    const total = rows.length;
    if (p.before !== undefined) rows = rows.filter((r) => r.seq < (p.before as number));
    const more = rows.length > limit;
    const page = rows.slice(0, limit);
    const last = page[page.length - 1];
    return latency({ rows: page, total, next_before: more && last ? last.seq : null });
  },

  async pii(): Promise<PiiReport> {
    const rank: Record<PiiState, number> = { flagged: 0, reviewed: 1, unscanned: 2, none: 3 };
    const entries = notes
      .filter((n) => n.front.pii !== "none")
      .map((n) => ({
        note: { id: n.front.id, name: n.front.name, ring: n.front.ring },
        state: n.front.pii,
        findings: n.front.name === "contact-notes-vendor-call" ? ([{ kind: "email", excerpt: "m***@vendor-example.com", line: 6, col: 14 }, { kind: "phone", excerpt: "+41 …87", line: 6, col: 52 }] as PiiFinding[]) : scanPii(n.body),
        // The write that set the state stamped `updated`; unscanned notes have no such write.
        reviewed_at: n.front.pii === "unscanned" ? null : n.front.updated,
      }))
      .sort((a, b) => rank[a.state] - rank[b.state] || a.note.name.localeCompare(b.note.name));
    return latency({ scan_enabled: true, entries, holds: [...holds.values()].map((h) => h.hold) });
  },

  async retention(): Promise<RetentionQueue> {
    const entries = retentionEntries();
    return latency({ entries, due: entries.filter((e) => e.due).length, indefinite: notes.filter((n) => !n.front.retention).length });
  },

  async applyRetention(dryRun, names): Promise<RetentionApplyReport> {
    if (names && names.length) {
      throw err(400, "bad-request", "applying retention to a subset (`names`) is not supported: the sweep erases every due note, and a partial sweep is not what the audit row `retention.expired` records; omit `names` to run the sweep, or use DELETE /api/v1/notes/{name} for a single note");
    }
    const all = retentionEntries();
    const removed: ForgetReport[] = [];
    const skipped: RetentionApplyReport["skipped"] = [];
    for (const e of all) {
      if (e.invalid !== undefined) {
        skipped.push({ name: e.note.name, reason: `retention is invalid and was not applied: ${e.invalid}` });
        continue;
      }
      if (!e.due) continue;
      const n = findNote(e.note.id);
      if (!dryRun) log(0, "retention", "retention.expired", `note:${n.front.id}`, { name: n.front.name, ring: n.front.ring, retention: e.retention, created: n.front.created, expired_at: e.expires_at, evaluated_at: new Date().toISOString() });
      removed.push(forgetImpl(n, dryRun, "retention-expired"));
    }
    return latency({ dry_run: dryRun, removed, skipped });
  },

  async modelCards(): Promise<ModelCard[]> {
    return latency([
      {
        role: "embedding",
        name: MODEL_NAME,
        source: MODEL_SOURCE,
        license: "MIT",
        hash: MODEL_HASH,
        format: "model2vec safetensors + tokenizer.json",
        dim: 256,
        pooling: "mean, L2-normalised",
        bytes: 31_457_280,
        intended_use: "Static token embeddings for hybrid recall over this store's Markdown blocks. Produces vectors; generates no text.",
        limitations: "No context sensitivity: a token embeds the same way in every sentence; English-centric vocabulary; not suitable for anything but retrieval ranking",
        verified_at: null,
        active: true,
      },
      {
        role: "inference",
        name: "qwen3:8b",
        source: INFERENCE_URL,
        license: "not stated",
        hash: null,
        format: "OpenAI-compatible HTTP",
        dim: null,
        pooling: null,
        bytes: null,
        intended_use: "Yes/no contradiction checks between retrieved blocks, ring and tag proposals for new notes, session summaries. Optional; every core function works without it.",
        limitations: "Heuristic judgement; a contradiction verdict is a marker for a human, not a resolution; every call is recorded in the audit log with token counts; the weights are not held by Cyberbrain and nothing about them is verified",
        verified_at: null,
        active: true,
      },
    ]);
  },

  async subjectAccess(identifier): Promise<SubjectAccessReport> {
    const q = identifier.trim().toLowerCase();
    if (!q) throw err(400, "bad-request", "`q` (the identifier) must not be empty");
    const hits: SubjectAccessReport["hits"] = [];
    let blocks = 0;
    for (const n of notes) {
      for (const b of n.blocks) {
        blocks++;
        if (b.text.toLowerCase().includes(q)) hits.push({ where: "block", ref: n.front.name, citation: b.citation, excerpt: b.text.slice(0, 160) });
      }
    }
    const idHash = fnv1a64(q);
    for (const r of events) {
      const byHash = r.action === "subject.access" && r.subject === `identifier:${idHash}`;
      if (byHash || r.subject.toLowerCase().includes(q) || JSON.stringify(r.detail).toLowerCase().includes(q)) {
        hits.push({ where: "audit", ref: `audit@${r.ts}`, citation: null, excerpt: `${r.ts} ${r.actor} ${r.action} ${r.subject}${byHash ? " (earlier request for this identifier)" : ""}` });
      }
    }
    const response_deadline = "one month from receipt (GDPR Art. 12(3)), extendable by two months for complex requests";
    log(0, OPERATOR, "subject.access", `identifier:${idHash}`, { profile: PROFILE, note_hits: hits.filter((h) => h.where === "block").length, audit_hits: hits.filter((h) => h.where === "audit").length, trimmed: 0, response_deadline });
    return latency({
      identifier: identifier.trim(),
      hits,
      searched: { notes: notes.length, blocks, audit_rows: events.length },
      caveats: ["matching is a case-insensitive substring search over block text and audit rows; a name spelled differently, abbreviated, or split across blocks is not found", "the audit log names the subject of a request by hash, not by identifier; earlier requests for this identifier are matched through that hash"],
      response_deadline,
    });
  },

  async doctor(): Promise<DoctorReport> {
    const names = byName();
    const findings: DoctorFinding[] = [];
    for (const n of notes) {
      for (const t of n.outbound) {
        if (names.has(t)) continue;
        if (VALID_NAME.test(t)) findings.push({ severity: "warn", check: "dangling-link", subject: n.front.name, message: `${n.front.name} links to [[${t}]] which does not exist yet (valid name: it names intent)` });
        else {
          // The server's `doctor_subject` has no arm for this check and falls back to "store".
          const normalised = t.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
          const hint = names.has(normalised) ? `; did you mean [[${normalised}]]?` : "";
          findings.push({ severity: "warn", check: "unresolvable-links", subject: "store", message: `${n.front.name} links to [[${t}]], which can never resolve: a note name must be a kebab-case slug${hint}` });
        }
      }
    }
    findings.push({ severity: "warn", check: "stale-index", subject: "index", message: "0 notes not indexed, 1 changed on disk since the last scan, 0 indexed notes whose file is gone; run `cyberbrain scan`" });
    const used = notes.filter((x) => x.front.ring <= 1).reduce((a, x) => a + x.blocks.reduce((b, y) => b + y.token_count, 0), 0);
    if (used * 10 >= RESIDENT_CAP * 8) findings.push({ severity: "warn", check: "ring-cap", subject: "r0+r1", message: `rings 0+1 hold ~${used} of ${RESIDENT_CAP} tokens (${Math.floor((used * 100) / RESIDENT_CAP)}%)` });
    for (const e of retentionEntries()) if (e.invalid !== undefined) findings.push({ severity: "warn", check: "retention", subject: e.note.name, message: `${e.note.name}: ${e.invalid}` });
    const due = retentionEntries().filter((e) => e.due).length;
    if (due) findings.push({ severity: "warn", check: "retention", subject: "store", message: `${due} note(s) past their retention; nothing expires by itself, run \`cyberbrain policy retention\`` });
    return latency({ ok: findings.length === 0, checked_at: new Date().toISOString(), findings, checks_run: ["notes tree", "stale index", "dangling links", "unresolvable links", "ring cap", "index integrity", "embedding profile", "audit chain", "retention"] }, 200);
  },

  async scan(full): Promise<ScanReport> {
    const ix = indexStats();
    const now = new Date().toISOString();
    lastScan = now;
    if (full) {
      lastFullScan = now;
      log(0, OPERATOR, "index.cleared", "index", { erased: { notes: ix.notes, blocks: ix.blocks, fts_rows: ix.fts_rows, vectors: ix.vectors, links_out: ix.links, links_in_unresolved: 0 }, dry_run: false });
    }
    const elapsed_ms = full ? 1840 : 62;
    const caveats: string[] = [];
    if (!full) caveats.push("blocks_written and vectors_written are the index totals after the scan; the scan reports how many notes it touched (see detail), not how many blocks");
    return latency(
      {
        full,
        elapsed_ms,
        scanned: notes.length,
        changed: full ? notes.length : 1,
        added: 0,
        removed: 0,
        blocks_written: ix.blocks,
        vectors_written: ix.vectors,
        dry_run: false,
        detail: {
          dry_run: false,
          full,
          files_listed: notes.length,
          indexed_new: full ? notes.length : 0,
          reindexed_changed: full ? 0 : 1,
          revectorised: 0,
          unchanged: full ? 0 : notes.length - 1,
          touched_only: 0,
          dropped_missing_file: [],
          skipped: [],
          links_written_back: full ? notes.length : 1,
          link_writeback_failed: [],
          oversized_blocks: [],
          embedder: { loaded: true, profile_id: PROFILE_ID, dim: 256, reason: null },
          profile_change: null,
          cleared: full ? { notes: ix.notes, blocks: ix.blocks, fts_rows: ix.fts_rows, vectors: ix.vectors, links_out: ix.links, links_in_unresolved: 0 } : null,
          index: ix,
          audit_preview: [],
          elapsed_ms,
        },
        caveats,
      },
      full ? 900 : 150,
    );
  },
};
