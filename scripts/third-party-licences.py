#!/usr/bin/env python3
"""Regenerate THIRD-PARTY-LICENSES.txt from cargo metadata.

    python3 scripts/third-party-licences.py > THIRD-PARTY-LICENSES.txt

Written down as a script rather than left as a command somebody once ran, because the file
had gone three releases out of date before anybody noticed: it is the sort of thing that is
only wrong when it matters.
"""
import collections, json, pathlib, subprocess
ROOT = pathlib.Path(__file__).resolve().parent.parent
md = json.loads(subprocess.check_output(
    ["cargo", "metadata", "--all-features", "--format-version", "1"], cwd=ROOT))
ours = {"cyberbrain","cyberbrain-core","cyberbrain-embed","cyberbrain-index",
        "cyberbrain-policy","cyberbrain-llm","cyberbrain-code","cyberbrain-desktop"}
groups = collections.defaultdict(list)
for p in md["packages"]:
    if p["name"] in ours: continue
    lic = p.get("license") or "(no licence declared)"
    url = p.get("repository") or p.get("homepage") or ""
    groups[lic].append((p["name"], p["version"], url))
total = sum(len(v) for v in groups.values())
out = []
out.append("Third-party licences")
out.append("====================")
out.append("")
out.append("Cyberbrain is licensed FSL-1.1-ALv2 (see LICENSE.md). It builds on the packages below,")
out.append("each under its own terms. Generated from cargo metadata with --all-features, so it lists")
out.append("more than a default build actually links. Regenerate with:")
out.append("")
out.append("    python3 scripts/third-party-licences.py > THIRD-PARTY-LICENSES.txt")
out.append("")
out.append(f"{total} packages, grouped by the licence they declare.")
out.append("")
for lic in sorted(groups):
    head = f"-- {lic} "
    out.append(head + "-" * max(0, 78 - len(head)))
    for name, ver, url in sorted(set(groups[lic])):
        out.append(f"   {name} {ver}  {url}".rstrip())
    out.append("")
print("\n".join(out).rstrip() + "\n", end="")
