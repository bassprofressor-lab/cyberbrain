import { dict, getLang, LOCALE } from "./i18n";

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
  return `${v < 10 ? v.toFixed(1).replace(".", getLang() === "de" ? "," : ".") : Math.round(v)} ${u[i]}`;
}

export function num(n: number | null | undefined): string {
  if (n === null || n === undefined) return "—";
  return new Intl.NumberFormat(LOCALE[getLang()]).format(n);
}

/**
 * "9 hr. ago" / "vor 9 Std." — Intl does the units and the word order, this only picks the
 * granularity, keeping the thresholds the dense layout was built around.
 */
/** A fixed number of decimals in the reader's notation: `5.7 cores` / `5,7 Kerne`. */
export function dec(n: number, digits = 1): string {
  return new Intl.NumberFormat(LOCALE[getLang()], { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(n);
}

export function relTime(ts: string | null | undefined, now = Date.now()): string {
  if (!ts) return dict().common.never;
  const t = Date.parse(ts);
  if (Number.isNaN(t)) return ts;
  const diff = t - now;
  const secs = Math.abs(diff) / 1000;
  let value: number;
  let unit: Intl.RelativeTimeFormatUnit;
  if (secs < 45) {
    value = Math.round(secs);
    unit = "second";
  } else if (secs < 3600) {
    value = Math.round(secs / 60);
    unit = "minute";
  } else if (secs < 86400) {
    value = Math.round(secs / 3600);
    unit = "hour";
  } else if (secs < 86400 * 30) {
    value = Math.round(secs / 86400);
    unit = "day";
  } else if (secs < 86400 * 365) {
    value = Math.round(secs / (86400 * 30));
    unit = "month";
  } else {
    value = Math.round(secs / (86400 * 365));
    unit = "year";
  }
  return new Intl.RelativeTimeFormat(LOCALE[getLang()], { numeric: "always", style: "narrow" }).format(diff < 0 ? -value : value, unit);
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

/** "P2Y" → "2 years" / "2 Jahre", "P90D" → "90 days" / "90 Tage". */
export function duration(iso: string | null | undefined): string {
  const t = dict();
  if (!iso) return t.common.indefinite;
  const m = /^P(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)W)?(?:(\d+)D)?(?:T.*)?$/.exec(iso);
  if (!m) return iso;
  const parts: string[] = [];
  const unit = (v: string | undefined, f: (n: number) => string) => {
    if (v) parts.push(f(Number(v)));
  };
  unit(m[1], t.units.year);
  unit(m[2], t.units.month);
  unit(m[3], t.units.week);
  unit(m[4], t.units.day);
  return parts.length ? parts.join(" ") : iso;
}

export function ringLabel(ring: number): string {
  return dict().rings.label[ring] ?? `r${ring}`;
}

export function ringHelp(ring: number): string {
  return dict().rings.help[ring] ?? "";
}

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
