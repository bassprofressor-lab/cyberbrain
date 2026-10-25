# Handing a period of the audit log to somebody else

```console
$ cyberbrain policy audit --since 2026-01-01T00:00:00Z --until 2026-03-31T23:59:59Z \
    --export q1-2026.jsonl
wrote 1284 row(s) to q1-2026.jsonl
checked as written: chain holds from anchor e143b7a271f6
```

The person who receives that file checks it with the binary and nothing else — no store, no
configuration, no network:

```console
$ cyberbrain verify-export q1-2026.jsonl
chain holds over 1284 row(s)
period:   2026-01-01T00:00:00Z to 2026-03-31T23:59:59Z
anchor:   e143b7a271f6a5517b7c80cb746ce15be91c2fe52493881eefbcf98c30155b04
last row: 0238e45f21154cbf53b852ae183efe8c2151f9b3a868571d9a9bb671c7e4eed4
written:  2026-09-07T14:18:16Z by cyberbrain 0.2.1
```

Exit code 0 when the chain holds, non-zero when it does not, and the message says which row
failed and how.

## Why a bundle and not just the rows

`verify` over the whole log starts at the genesis marker. A period taken out of the middle
has no genesis in it, so a bare list of rows can be checked for internal consistency and
nothing else — and "internally consistent" is exactly what a forged file also is.

The bundle adds two things:

- **The anchor**: the hash the first row points back at. Checking starts there, so the rows
  have to be the continuation of something, not a story that begins wherever it likes.
- **The footer**: the row count and the last hash. A file with rows removed from the end
  fails instead of answering with a shorter period.

**What this proves, exactly:** the extract is internally intact and begins where it says it
begins. Whether that anchor belongs to the machine's real history is a question only the full
log answers. Say it that way in a contract, and it holds.

## The format

One JSON object per line: header, rows, footer.

```json
{"kind":"cyberbrain.audit.export","version":1,"tool":"cyberbrain 0.2.1","exported_at":"…","from":"…","to":"…","anchor":"…","rows":3}
{"ts":"…","actor":"operator","action":"note.write","subject":"note:01M1…","detail":{"…":"…","_chain":{"prev":"…","hash":"…","at":"…"}}}
{"kind":"cyberbrain.audit.export.end","rows":3,"first_hash":"…","last_hash":"…"}
```

`from` and `to` are absent when that end was unbounded. `anchor` is the literal string
`genesis` when the extract starts at the beginning of the log.

### The rule for the hash

```
blake3( prev "\n" at "\n" actor "\n" action "\n" subject "\n" detail-without-_chain )
```

`detail-without-_chain` is the `detail` object with the `_chain` key removed, serialised as
compact JSON with sorted keys and no spaces. Rust's `serde_json` does that by default;
most other languages have to be asked. That one line is the whole interoperability question,
and getting it wrong looks like every row failing at once.

`ts` is deliberately **not** in the hash: the index writes its own timestamp on read-back,
and a chain that depended on it would break for a reason that has nothing to do with
tampering. The chain's own timestamp is `_chain.at`.

### A selection: `--action`, `--subject`

```console
$ cyberbrain policy audit --action note.erase --export erasures.jsonl
```

The rows of one action are not neighbours in the chain, so a selection cannot be walked as
one. Such a file is written as **version 2** and its header names the selection:

```json
{"kind":"cyberbrain.audit.export","version":2,…,"anchor":"…","rows":4,"filter":{"action":"note.erase"}}
```

It is checked row by row, each row against the hash it names as its predecessor (`_chain.prev`),
and every row has to belong to the selection. **What this proves:** each row is exactly as it
was written. **What it cannot prove:** that the selection is complete, because the rows between
them are not in the file. A full period proves both; hand over the full period when completeness
is the question. A hub refuses a selection as a delivery for the same reason.

A plain period without a selection is still written as version 1, byte for byte as before, so
a reader that only knows version 1 keeps working.

## Signed exports

With an audit key (`cyberbrain policy audit-key --init`), a bundle can be signed:

```console
$ cyberbrain policy audit --since 2026-01-01T00:00:00Z --export q1.jsonl --signed
wrote 1284 row(s) to q1.jsonl
checked as written: chain holds from anchor e143b7a271f6
signed with key 657eda56312236f3e924aed039d9d501

$ cyberbrain verify-export q1.jsonl --expect-key 657eda56312236f3e924aed039d9d501
```

A signed bundle is version 2. The header names the public key (`key`), and the footer carries
`sig`: Ed25519 over `cyberbrain.audit.export.v2\n` followed by **every byte of the file before
the footer line**, exactly as written. No JSON rule is involved, so convert nothing (line endings
included) before checking.

**Without `--expect-key` a signature proves only that somebody signed** — a forger signs too.
The fingerprint is what turns it into evidence, so hand it over on a separate path the store's
owner cannot change later: a contract, a ticket, the auditor's own file. With `--expect-key`,
the bundle and every signed row in it must carry that key.

Rows in a signed store are themselves signed (chain format 2, `_chain.v = 2`, see below), so a
bundle may mix them with the unsigned rows written before the key existed. Those are reported
as unsigned: covered by the chain, not by a signature. An unsigned row *after* a signed one is
refused.

### The rule for a signed row (chain format 2)

```
blake3( len "cyberbrain.audit.v2" | len prev | len at | len actor | len action | len subject | len detail-without-_chain )
```

where `len x` is the byte length of `x` as an 8-byte little-endian integer, followed by `x`
itself. Format 1 separated the fields with a newline, and a field may contain one: `actor
"a\nb", action "c"` and `actor "a", action "b\nc"` hashed alike. Rows of format 1 keep their rule;
only signed rows use this one. `_chain.sig` is Ed25519 by `_chain.key` over
`cyberbrain.audit.row.v2\n` followed by the hash in hex.

## Erasure reports

```console
$ cyberbrain forget old-customer-note --report erasure.json --sign
$ cyberbrain verify-export erasure.json --expect-key 657eda56312236f3e924aed039d9d501
```

One JSON object: what was removed, `residue` (whether pre-erasure pages may remain in the WAL,
with SQLite's numbers under `erasure.scrub`), and the chain hashes of the `note.erase.requested`
and `note.erase.completed` rows, so it can be tied to an audit bundle. Signed, it carries `key`
and `sig`: Ed25519 over `cyberbrain.erasure.report.v1\n` followed by the object without `sig`,
as compact JSON with sorted keys.

## Checking it without Cyberbrain

The retention period for this evidence is measured in years, and no program is a safe
dependency for that long. So the check has to be small enough that anyone can rewrite it:

```console
$ pip install blake3 cryptography
$ python3 scripts/verify-audit-export.py q1-2026.jsonl [--expect-key FINGERPRINT]
```

(`cryptography` is only needed for signed files; without it the script refuses a signed file
rather than skipping the signature.) That script is under two hundred lines, it is in this repository, and CI runs it against a bundle
this binary just wrote. If the two ever disagree, one of them is wrong about the rule above —
which is the point of having two.

## Things worth knowing before you rely on it

- **Both period bounds are inclusive.** Timestamps in the log have millisecond resolution and
  several rows can share one, so a boundary can land in the middle of a group. Inclusive
  bounds mean an export takes one row too many rather than one too few.
- **An export with no `--limit` covers the whole period.** The listing still defaults to 50
  rows; a file does not, because a silently truncated period is the one mistake the recipient
  cannot see.
- **An empty period is a valid bundle.** "Nothing happened that week" is an answer an auditor
  may need, and it verifies like any other.
- **The export is itself recorded** in the log, after the rows have been rendered — so the
  file does not contain its own export row, but the next one will. An extract is an access,
  and an access that leaves no trace is the one nobody can ask about later.
- **Rows written by the index have no chain.** The index writes its own `index.*` rows into
  the same table; a mixed log verifies up to the first of those and says so rather than
  skipping it, because silently ignoring unchained rows is how an edited row would hide.
