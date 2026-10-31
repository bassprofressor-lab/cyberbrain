// CHANGELOG.md is rendered from the repository root. Its relative links (LICENSE.md, docs/HUB.md, …)
// point at files that are not part of the site, so send them to the file on GitHub.
import { readFileSync, writeFileSync } from "node:fs";

const REPO = "https://github.com/bassprofressor-lab/cyberbrain/blob/main/";
for (const page of ["dist/changelog/index.html", "dist/de/changelog/index.html"]) {
  const html = readFileSync(page, "utf8");
  const out = html.replace(/href="([A-Za-z0-9_][A-Za-z0-9_./-]*\.(?:md|toml))"/g, (_, f) => `href="${REPO}${f}"`);
  writeFileSync(page, out);
}
