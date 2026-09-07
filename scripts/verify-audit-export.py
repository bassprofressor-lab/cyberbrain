#!/usr/bin/env python3
"""Check a Cyberbrain audit export without Cyberbrain.

This exists to prove a claim the format makes: everything needed to verify an export is in
the file, and the check is small enough that anyone can rewrite it. A retention period runs
for ten years; this program will not, and the evidence must not depend on it.

    $ pip install blake3
    $ python3 verify-audit-export.py export.jsonl

Exit code 0 when the chain holds, 1 when it does not, 2 when the file is not an export.

The rule, in one sentence: each row's hash is blake3 over `prev`, the chain timestamp, the
actor, the action and the subject — each followed by a newline — and then the detail object
without its `_chain` key, serialised as compact JSON with sorted keys.
"""

import json
import sys

try:
    from blake3 import blake3
except ImportError:
    sys.exit(
        "needs the blake3 module: pip install blake3\n"
        "(any blake3 implementation will do; the format does not depend on this one)"
    )

KIND = "cyberbrain.audit.export"
KIND_END = "cyberbrain.audit.export.end"
VERSION = 1


def canonical(detail):
    """The detail as it was hashed: no `_chain`, compact separators, keys in order.

    Rust's serde_json writes object keys sorted and without spaces. Python's json does
    neither by default, so both have to be asked for — this line is the whole of the
    interoperability question, and getting it wrong shows up as every row failing.
    """
    d = {k: v for k, v in detail.items() if k != "_chain"}
    return json.dumps(d, separators=(",", ":"), sort_keys=True, ensure_ascii=False)


def row_hash(row, prev):
    chain = row["detail"]["_chain"]
    h = blake3()
    for part in (prev, chain["at"], row["actor"], row["action"], row["subject"]):
        h.update(part.encode("utf-8"))
        h.update(b"\n")
    h.update(canonical(row["detail"]).encode("utf-8"))
    return h.hexdigest()


def main(path):
    lines = [l for l in open(path, encoding="utf-8").read().splitlines() if l.strip()]
    if not lines:
        return 2, "file is empty"

    header = json.loads(lines[0])
    if header.get("kind") != KIND:
        return 2, f"first line is {header.get('kind')!r}, expected {KIND!r}"
    if header.get("version") != VERSION:
        return 2, f"format version {header.get('version')} is not {VERSION}"

    footer = json.loads(lines[-1])
    if footer.get("kind") != KIND_END:
        return 2, "last line is not a footer; the file is truncated"

    rows = [json.loads(l) for l in lines[1:-1]]
    if header["rows"] != len(rows):
        return 1, f"header says {header['rows']} rows, file carries {len(rows)}"
    if footer["rows"] != len(rows):
        return 1, f"footer says {footer['rows']} rows, file carries {len(rows)}"

    prev = header["anchor"]
    for i, row in enumerate(rows, start=1):
        chain = row.get("detail", {}).get("_chain")
        if not chain:
            return 1, f"row {i} carries no _chain"
        if chain["prev"] != prev:
            return 1, f"row {i}: prev does not match the row before it"
        want = row_hash(row, prev)
        if chain["hash"] != want:
            return 1, f"row {i}: content does not match its hash; the row was edited"
        prev = chain["hash"]

    if rows and footer.get("last_hash") != rows[-1]["detail"]["_chain"]["hash"]:
        return 1, "footer's last hash is not the last row's hash"

    period = (
        f"{header.get('from', 'the beginning')} to {header.get('to', 'the end')}"
        if ("from" in header or "to" in header)
        else "the whole log"
    )
    return 0, (
        f"chain holds over {len(rows)} row(s)\n"
        f"period:   {period}\n"
        f"anchor:   {header['anchor']}\n"
        f"written:  {header['exported_at']} by {header['tool']}"
    )


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("usage: verify-audit-export.py <export.jsonl>")
    code, message = main(sys.argv[1])
    print(message)
    sys.exit(code)
