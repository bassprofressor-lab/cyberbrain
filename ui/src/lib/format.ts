export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  if (n < 1024) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < u.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
}

export function num(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  return new Intl.NumberFormat("en-GB").format(n);
}

export function relTime(ts: string | null | undefined, now = Date.now()): string {
  if (!ts) return "never";
  const t = Date.parse(ts);
  if (Number.isNaN(t)) return ts;
  const d = now - t;
  const future = d < 0;
  const a = Math.abs(d);
  const s = Math.round(a / 1000);
  let out: string;
  if (s < 45) out = `${s}s`;
  else if (s < 3600) out = `${Math.round(s / 60)} min`;
  else if (s < 86400) out = `${Math.round(s / 3600)} h`;
  else if (s < 86400 * 30) out = `${Math.round(s / 86400)} d`;
  else if (s < 86400 * 365) out = `${Math.round(s / (86400 * 30))} mo`;
  else out = `${(s / (86400 * 365)).toFixed(1)} y`;
  return future ? `in ${out}` : `${out} ago`;
}

export function absTime(ts: string | null | undefined): string {
  if (!ts) return "—";
  const t = Date.parse(ts);
  if (Number.isNaN(t)) return ts;
  const d = new Date(t);
  const p = (x: number) => String(x).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

export function shortDate(ts: string | null | undefined): string {
  return absTime(ts).slice(0, 10);
}

/** "P2Y" → "2 years", "P90D" → "90 days". */
export function duration(iso: string | null | undefined): string {
  if (!iso) return "indefinite";
  const m = /^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)W)?(?:(\d+)D)?(?:T.*)?$/.exec(iso);
  if (!m) return iso;
  const parts: string[] = [];
  const unit = (v: string | undefined, name: string) => {
    if (!v) return;
    parts.push(`${v} ${name}${v === "1" ? "" : "s"}`);
  };
  unit(m[1], "year");
  unit(m[2], "month");
  unit(m[3], "week");
  unit(m[4], "day");
  return parts.length ? parts.join(" ") : iso;
}

export const RING_LABEL: Record<number, string> = {
  0: "Invariant",
  1: "Protocol",
  2: "Knowledge",
  3: "Session",
  4: "External",
};

export const RING_HELP: Record<number, string> = {
  0: "Operator invariants. Hard rules. Override all higher rings; always injected.",
  1: "Operating protocol and handoff state. Override 2+; always injected.",
  2: "Curated project knowledge. Retrieved on demand.",
  3: "Session records and observations. Retrieved on demand, lower weight.",
  4: "Imported or unverified material. Retrieved last, marked unverified.",
};

export function highlight(text: string, query: string): Array<{ t: string; hit: boolean }> {
  const terms = query
    .toLowerCase()
    .split(/[^a-z0-9.\-]+/)
    .filter((t) => t.length > 1);
  if (!terms.length) return [{ t: text, hit: false }];
  const src = terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|");
  const splitter = new RegExp(`(${src})`, "gi");
  const tester = new RegExp(`^(?:${src})$`, "i");
  return text
    .split(splitter)
    .filter((s) => s !== "")
    .map((s) => ({ t: s, hit: tester.test(s) }));
}
