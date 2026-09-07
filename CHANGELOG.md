# Changelog

What changed, in the words of someone who has to explain it to a user rather than to a
compiler. Notable changes only; the commit history has the rest, and it is written to be read.

This file is also the record of when each version shipped, which the licence needs: under
[FSL-1.1-ALv2](LICENSE.md) every release turns Apache-2.0 two years after **its own** release
date, so that date has to survive somewhere more durable than a tag that can be moved.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **`cyberbrain hub`: one machine collects the audit rows of the others.** Register a device
  and it gets a token; it delivers the bundles from `policy audit --export`, and the hub
  checks each one against the chain it already has from that device before keeping it. A
  delivery that overlaps is fine and contributes only what is new; one that skips rows is
  refused with both hashes named. `hub fleet` shows devices, versions and when each was last
  heard from — state, not activity. The record is append-only in the database, like the
  store's own audit log. See [`docs/HUB.md`](docs/HUB.md).

- **Licences.** A hub collects only with a current one: signed, checked offline against a key
  compiled into the binary, no call home. Seats are devices and are counted at enrolment.
  From 30 days out every view that shows state says so; after the end date the hub stops
  accepting rows **and does nothing else** — the record stays readable and exportable,
  clients buffer, deliveries get a `503` that says to keep buffering, and renewing takes what
  they held with the chain unbroken. `hub licence keygen` and `issue` are the issuer's side.
- **Invitations.** `hub add --invite` writes a file with the token, the hub address and the
  shared inference endpoint, so a machine is set up from one file instead of three settings
  typed by hand.

  This is a second surface, not `serve` with its bind opened: `serve` stays loopback and
  unauthenticated because there is nothing remote to authenticate (SPEC §8.2), and it can
  read, write and delete notes. The hub authenticates and can only take rows — it has no
  notes, no index and no search.

- **A period of the audit log as a file somebody else can check.**
  `cyberbrain policy audit --since … --until … --export period.jsonl` writes the rows with an
  anchor and a footer, and `cyberbrain verify-export period.jsonl` checks one with no store,
  no configuration and no network. The format and the hash rule are written down in
  [`docs/AUDIT-EXPORT.md`](docs/AUDIT-EXPORT.md), and `scripts/verify-audit-export.py` is a
  second implementation of the check — eighty lines, run by CI against a real bundle, because
  evidence that only one program can verify is not evidence for very long.
- `--since` and `--until` on `policy audit`, so a period can be asked for at all. Both bounds
  are inclusive: timestamps have millisecond resolution and rows can share one, and an export
  should take a row too many rather than one too few.

### Changed

- `policy audit --limit` no longer defaults to 50 for an export. The listing keeps the
  default; a file without an explicit limit covers the whole period, because a silently
  truncated period is the one mistake its recipient cannot see.

## [0.2.1] — 2026-09-07

### Changed

- Starting the Windows launcher while it is already running no longer gives you a second
  tray icon and a second server on a second port. The one that is running leaves its address
  behind, and the second one opens your browser at it and exits. That address is used only if
  it is loopback: it comes from a file in your own profile, and anything able to write there
  would otherwise get to choose where the browser goes.

## [0.2.0] — 2026-09-07

### Added

- **A way into Cyberbrain on Windows that is not a terminal.** The release carries
  `cyberbrain-setup-<version>.exe`, which installs the command-line tool plus a launcher and
  puts Cyberbrain in the Start menu. The launcher asks once which project to open, starts the
  server on a port Windows picks, opens the browser at it, and lives in the notification area
  until quit; a job object makes sure the server goes when it does, even when it is killed
  rather than asked. Not a second implementation of anything: the page and the API are the
  ones `cyberbrain serve` has always served.

### Fixed

- `cyberbrain serve --no-open` did nothing at all. The flag had been in the help since the
  command existed, promising not to do something that never happened; `serve` now opens the
  address it just bound, and `--no-open` is how you stop it. On a machine with nothing to
  open, it says so and carries on serving.

## [0.1.0] — 2026-09-06

The first published version.

### Added

- `cyberbrain policy obligations`, `GET /api/v1/policy/obligations` and a section on the
  compliance screen: the catalogue of what the active profile claims the law says, each line
  with its article and how sure the author is of it. The list had existed since the first
  profile and was read by nothing but a test.
- The web UI speaks German as well as English, chosen from the browser on the first visit and
  remembered after that. Numbers, dates and durations follow the language.
- `inference.contradiction_budget_ms` (default 3 s): how long a recall waits for the
  contradiction check before handing the hits over unchecked, with the reason in the caveats.
  On the machine this was found on, a recall took 1 minute 44 seconds and spent 1.5 of it on
  the CPU.
- `scripts/recall-eval.py`, a harness that measures recall quality against a labelled query
  set, so a change to retrieval can be shown to have helped instead of argued about.
- `CONTRIBUTING.md` and `SECURITY.md`.

### Changed

- Lexical search matches on prefixes from four characters up: `postgres` now reaches
  `PostgreSQL`, which the tokenizer's lack of stemming used to prevent.
- The note name is indexed, so a note whose distinguishing word lives only in its title can
  be found by that word. Schema v2; an existing store migrates on the next open, no rescan.
- `cyberbrain policy audit --verify` exits non-zero when the chain is broken. It printed
  "CHAIN BROKEN" and exited 0, so a nightly check could not fail.
- `forget` no longer reports "0 vectors" as a possible silent failure on a store that does
  not embed, where it was never true and fired on every note.
- The usage charts keep a fixed height instead of scaling with the window, and their axes
  round to numbers a person recognises.
- The web page can be embedded from `crates/cyberbrain/ui-dist` as well as `ui/dist`, which
  is what makes it possible for a published crate to carry it at all.

### Fixed

- The audit chain reports the broken row the way a reader counts rows, from one.
- The ring-cap error carried ten stray spaces from a wrapped string literal.

Cited, trust-tiered, local-first memory for AI coding agents, as described in
[`docs/SPEC.md`](docs/SPEC.md). Seven crates on crates.io; binaries follow from the release
workflow when a tag is pushed.

[Unreleased]: https://github.com/bassprofressor-lab/cyberbrain/compare/main...HEAD
