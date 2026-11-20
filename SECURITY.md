# Security

## Reporting

Use GitHub's **private vulnerability reporting** on this repository (Security → Report a
vulnerability). If that is not available to you, write to **info@krynexlabs.de** with
`cyberbrain` in the subject.

Please do not open a public issue first. Cyberbrain is used as a memory of other people's
projects; a working exploit posted publicly is read by more attackers than maintainers.

Include what you would want to receive: what you did, what happened, what you expected, and
the version (`cyberbrain --version`) or commit. A failing test or a shell transcript is worth
more than a paragraph.

## What this project treats as a vulnerability

Cyberbrain makes a small number of promises that are checkable, and breaking any of them is a
security issue rather than a bug:

- **Data leaves the machine on a path that is not in the egress register.** The register is
  closed at compile time and `cyberbrain policy egress` prints it. Anything outbound that is
  not on that list, including a lookup, a preconnect or a redirect chain, belongs here.
- **The audit log of a store with an audit key can be changed without `policy audit
  --verify` noticing.** Each row is chained to its predecessor (blake3) and signed with the
  store's Ed25519 key; a new store gets its key at `init` (since 0.10.8). Rows missing at
  the end, or an older copy of `audit.db` put back, are checked against the head anchor that
  every append moves forward in the user's state directory, outside the store. An export is
  evidence only when checked against the expected key: `verify-export --expect-key
  <fingerprint>` exits 0, without `--expect-key` it exits 4 ("consistent, not evidence").
  Within those limits, a modification, deletion, reordering, truncation or re-signing that
  verifies clean is a finding.
- **`forget` leaves something behind.** File, blocks, vectors, FTS rows, links and
  derivatives go in one transaction. A copy that survives in any cache is a finding.
- **The PII gate can be walked past.** Under profile `eu`/`ch` a write with findings is held.
  Whoever writes decides how it goes on: redact, or `--force` (MCP `force: true`), which
  writes the note marked `pii: flagged` for the operator to see; a redaction an agent or hook
  chose is marked `flagged` too, never `reviewed`. A path that writes without one of those
  decisions and its mark, or that puts the matched text into the audit log, is a finding.
- **An agent writes or erases rings 0 and 1 as the operator** through a surface that is
  meant to refuse it: `serve` without `--allow-resident-writes` (its note routes and its
  command line alike), or, on Linux, a CLI or daemon call from a process that still descends
  from the agent's session. 0.9.1 closed one of each (see the changelog).
- **`serve` is reachable from outside the machine.** It binds loopback and has no
  authentication; that is only safe because it is loopback. A binding, redirect or header
  that changes this is a finding.
- **The model artefact is loaded despite a hash mismatch**, or a model is fetched without the
  configured consent.
- Anything that makes a result **look more checked than it is**: a caveat that should have
  been emitted and was not, a citation that resolves to the wrong block, a contradiction that
  is silently resolved.

Ordinary crashes, wrong results and unhelpful errors are welcome as normal issues.

## What this project does not treat as a vulnerability

- Anyone with read access to the store can read the notes. The store is plain Markdown on
  your disk; it is not encrypted at rest and does not claim to be.
- Anyone who can run code as your user can do everything the binary can do.
- Telling an agent from the operator is attribution, not authentication. An agent that
  detaches a process from its session (`setsid -f`, a double fork, `systemd-run`, cron), or
  hands work to something the operator already runs outside it, is the operator to this
  program; on macOS and Windows so is one that drops `CLAUDECODE`. Where that matters, the
  boundary with credentials is the hub or AgentGuard.
- **The limits of the audit log, said plainly.** A store made with `init --no-audit-key`
  (or before 0.10.8 without `policy audit-key --init`) has a hash chain only: whoever can
  write to the store can rewrite and re-hash it, and the check shows that the file is
  consistent, not who wrote it. The private key lies in the store (`keys/audit.ed25519`,
  0600) and the head anchor in the user's state directory, so a process running as the same
  user can read the one and rewrite the other. The hooks refuse an agent's tools to read,
  copy or write the key and to make it (`policy audit-key --init` is the operator's), but
  that is attribution, not a sandbox (see above). Against the same user, and against a
  restored backup of the whole home directory, only a record on another machine helps: a
  hub or a timestamping service that countersigns the head. That is not built yet.
- The PII scan misses something. It is documented as heuristic, a seatbelt rather than a
  guarantee, and it says so on every screen that shows it.
- The inference endpoint you configured yourself sees the note text you asked it to check.
  That path is registered, audited per call, and printed by `policy egress`.

## What happens next

This is a small project, maintained alongside other work. You will get an
acknowledgement that a person read it. There is no bounty and no response-time guarantee, and
promising one here would be a claim we could not keep.

If a report leads to a fix, you get credit in the release notes unless you would rather not.
Where a fix changes a promise in the README or the specification, the change is written down
there too, because a promise that quietly narrows is worse than one that was never made.
