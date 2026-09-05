// SPEC §2: every dependency permissive; no GPL/AGPL, direct or transitive. Walks the
// installed tree (the lockfile-resolved reality, not the manifest) and fails on a hit.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const ALLOWED = /^(MIT|ISC|BSD-2-Clause|BSD-3-Clause|Apache-2\.0|MPL-2\.0|0BSD|CC0-1\.0|Unlicense|BlueOak-1\.0\.0|Python-2\.0|CC-BY-4\.0)$/;
const root = new URL("../node_modules", import.meta.url).pathname;
const bad = [];
const seen = new Map();

function walk(dir) {
  let entries;
  try {
    entries = readdirSync(dir);
  } catch {
    return;
  }
  for (const e of entries) {
    if (e.startsWith(".")) continue;
    const p = join(dir, e);
    if (e.startsWith("@")) {
      walk(p);
      continue;
    }
    let pkg;
    try {
      pkg = JSON.parse(readFileSync(join(p, "package.json"), "utf8"));
    } catch {
      continue;
    }
    const lic = typeof pkg.license === "string" ? pkg.license : pkg.license?.type ?? (Array.isArray(pkg.licenses) ? pkg.licenses.map((l) => l.type).join(" OR ") : "UNKNOWN");
    seen.set(`${pkg.name}@${pkg.version}`, lic);
    const parts = lic.replace(/[()]/g, "").split(/\s+(?:OR|AND)\s+/i);
    const ok = lic.includes(" OR ") ? parts.some((x) => ALLOWED.test(x)) : parts.every((x) => ALLOWED.test(x));
    if (!ok) bad.push(`${pkg.name}@${pkg.version}: ${lic}`);
    try {
      if (statSync(join(p, "node_modules")).isDirectory()) walk(join(p, "node_modules"));
    } catch {
      /* none */
    }
  }
}
walk(root);
const counts = {};
for (const l of seen.values()) counts[l] = (counts[l] ?? 0) + 1;
console.log(`${seen.size} packages:`, counts);
if (bad.length) {
  console.error("DISALLOWED LICENCES:\n  " + bad.join("\n  "));
  process.exit(1);
}
console.log("all permissive");
