# Changelog

What changed, in the words of someone who has to explain it to a user rather than to a
compiler. Notable changes only; the commit history has the rest, and it is written to be read.

This file is also the record of when each version shipped, which the licence needs: under
[FSL-1.1-ALv2](LICENSE.md) every release turns Apache-2.0 two years after **its own** release
date, so that date has to survive somewhere more durable than a tag that can be moved.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] — 2026-09-07

### Added

- **The hub has a page.** `http://localhost:7788/` on the machine it runs on, answering the
  three questions somebody has the day they install one: is this a hub, was the licence
  accepted, is anything reporting. A licence file lying next to the record is one button; a
  box to paste one into is there as well; registering a machine writes its invitation next to
  the record and says where. Everything on it was already answerable by typing, and that was
  the problem — a product whose state can only be read at a prompt has no state as far as its
  owner is concerned.

  Loopback only: it shows who is on the network and can install a licence, on a port the
  whole network can reach. Rather than invent a sign-in for this slice, the rule is that you
  have to be at the machine. `/api/v1/fleet` is now under the same rule, where it previously
  had no authentication at all. `/health` and `/api/v1/ingest` are unchanged.

  Server-rendered, no JavaScript: the store's web UI is a built bundle behind a feature flag
  and building it needs node, which must not be the price of finding out whether a licence
  was accepted.

- **A sign-in for the hub's page, and the page reachable from a desk.** The account is
  `admin` and there is no default password: the first visit from the machine the hub runs on
  sets one, which is safe unauthenticated because whoever is at that console could read the
  record with any SQLite tool anyway. After that the page and `/api/v1/fleet` are reachable
  from anywhere on the network. `cyberbrain hub admin reset` is the way back in when somebody
  leaves. argon2 with a per-hub salt, sessions in memory so a restart signs everybody out, and
  a fixed delay on a wrong password rather than a lockout — locking out an administrator is a
  way to take a hub away from the person who runs it. It is not the roles model: nothing
  reachable with this password can read an activity row.

- **The dashboard says whether this machine reports to a hub.** A line in the sidebar: either
  "this machine keeps everything to itself", or the hub it delivers to, when it last did, and
  a link to it. Somebody installing a hub asked how to tell any of that from the dashboard,
  and the honest answer was that you could not — the answer was a command, which is no answer
  for the person the collection is *about*. The last delivery is read out of the store's own
  audit log rather than a note kept on the side, so the two cannot disagree.
  `GET /api/v1/hub` is the route.

### Fixed

- **A byte order mark stopped a licence from being accepted.** Notepad and a good many mail
  clients write one when they save UTF-8. It is invisible, it sits three bytes in front of
  the JSON, and it turned a perfectly good licence into `first line is not a licence`. The
  signature covers the canonical line, which never included the mark, so removing it restores
  the bytes that were signed rather than excusing anything. CRLF needed no help.
- **A licence file that could not be read looked exactly like no licence file at all.**
  "Save as > Unicode" writes UTF-16, and the read failed silently — so the hub reported that
  nothing had been dropped in, next to a file the person could see. It now says which file it
  could not read and what to save it as.

- **A hub with no licence did not say where it had looked for one.** The log said `no licence
  installed` and stopped, which leaves the reader unable to tell whether the file was looked
  for, looked for in another directory, or found and rejected. It now names the directory and
  the filename when there is no licence at all, while staying quiet for a hub that was
  licensed months ago and has no file lying about.
- **`licence.txt.txt` and `license.txt` are accepted.** Explorer hides known extensions, so
  saving an attachment as `licence.txt` produces the first of those and displays it as the
  name that was intended — the mistake is invisible to the person making it. The log always
  says which name was used, and the documented one still wins when both are present.

- **A failed service registration said nothing usable.** `cannot register the service: IO
  error in winapi call` was the whole message: `windows_service::Error::Winapi` displays like
  that and keeps the operating system's sentence and number one level down in `source()`.
  Every message from this module now carries both, so it can be looked up and quoted, and
  access denied and "being removed" each say what to do about them.
- **Installing over an existing service failed instead of updating it.** Reinstalling,
  repairing and upgrading are the ordinary way this gets run, and all three hit it. An
  existing registration is now updated to the new paths and address and started — an upgrade
  that kept the old command line would run yesterday's executable from a directory that may
  no longer exist. Already running counts as started.

- **The installer CI offers for testing had no web page in it.** That job built
  `--no-default-features`, which is a supported build and is not the product: clicking the
  Start menu entry opened a browser on `{"code":"not-found"}` where a program should have
  been. It builds the page now, like the release workflow does. An artefact put in front of
  people to try has to be the thing they would get.
- **The launcher opened a browser at a build with no page.** Even with the above fixed, that
  build exists — `cargo install --no-default-features` is documented. `serve` now says so on
  the stream the launcher already reads, before the address, and the launcher stops with a
  sentence naming the cause instead of pointing a browser at a JSON error. The two constants
  live in two programs on purpose, because the launcher runs whichever `cyberbrain` is beside
  it; a test compares them so they cannot drift.

### Added

- **The hub runs as a Windows service, and setting it up is a tick box.** The installer has
  an optional **Hub service (collector)** component — off by default, because most machines
  are clients and there it would open a port for nothing. Ticking it creates
  `C:\ProgramData\Cyberbrain\`, registers the service to start automatically on
  `0.0.0.0:7788`, opens that port in Windows Firewall (without which the hub listens and
  nothing ever arrives, which looks exactly like every client being broken), and adds a
  Start menu shortcut to the folder.

  **Licensing it is copying a file**: save the licence as
  `C:\ProgramData\Cyberbrain\licence.txt` and restart the service. It is read on start and
  what it found goes into `hub-service.log` beside the record — a service has no console, so
  anything printed to stdout is a message nobody will ever read, including the one saying why
  it will not start. `cyberbrain hub service install|uninstall|start|stop|status` is the same
  thing for administrators who would rather see a command; `status` exits non-zero when the
  service is not running.

  `hub serve` is one executable in both roles: started by the service control manager it
  behaves as a service, started from a prompt it is an ordinary console server. No flag to
  remember, and no second binary to drift from the first. Uninstalling removes the service
  and the firewall rule and **leaves the record alone** — deleting the software must never
  delete the evidence it was collecting.
- **"Connect to the company hub…" in the desktop launcher's menu, and delivery without a
  scheduled task.** The invitation arrives as an email attachment; this opens it and enrols
  the machine, and says afterwards that notes stay local. After that the launcher delivers on
  its own — half a minute after it starts, every quarter of an hour, and once on the way out
  — off the message pump, one attempt at a time, and silently: a train or a hotel wifi is not
  news, and the hub's fleet view is where a machine that stopped reporting shows up. A store
  that belongs to no hub is asked once and then left alone. Without this, enrolling from the
  menu would have collected nothing until somebody set up a scheduled task, which is the
  command prompt coming back in through the window.

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
- **Roles, and a two-person rule for reading activity.** An administrator sees state and
  never activity; an auditor may ask, naming a reason; a countersigner — the works council or
  a named second person — approves, and cannot approve their own request. Approval opens a
  window that closes itself, and extending means asking again so the reason is stated again.
  Every step is a row in the hub's own hash chain: `hub access-log` answers who looked, why,
  who approved it, and whether anything was removed afterwards.

  What it enforces is the route. It does not defend against someone with file access to the
  hub's database, and the documentation says so: a works agreement should describe this as a
  procedure supported by software, not a guarantee made by it.
- **A fleet view that leads with what is wrong, a check of the whole record, and a report
  you can hand over.** `hub fleet` sorts devices with concerns first and names them: never
  reported, quiet for so many hours, last delivery refused, running an older version than the
  hub. Refused deliveries are remembered, without which a gap would be invisible — it leaves
  no rows, so the device would look merely quiet. `hub verify` re-derives every chain from
  the rows as they are on disk now, which is the question a restored backup raises and the
  append-only triggers do not answer. `hub report --out-dir` writes one verifiable file per
  device for a period, plus a summary; each file is checked as it is written and verifies
  with `verify-export` or the Python script.
- **`audit-sync`, the third registered egress purpose, and the client side that uses it.**
  `cyberbrain hub enrol <invitation>` points a store at a hub; `cyberbrain hub push` delivers
  its audit rows, meant for a timer. The path appears in `cyberbrain policy egress` whether
  or not a store is enrolled, states that it carries no note content, and the gate refuses
  any destination that is not the hub this store enrolled with — so editing the URL produces
  a refusal, which is itself an audited row.

  The token is kept outside the store, because `cyberbrain.toml` lives in a repository.
  Nothing is buffered separately either: the audit log already holds every row in order, so
  a failed delivery changes nothing and the next one covers the same ground.
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

### Fixed

- **SPEC §12.1 described a TLS arrangement this program does not have.** It said no root
  store was available and that `https://` failed closed unless the operator supplied a CA;
  the client has been using the platform's certificate store all along, and a public HTTPS
  host answers on the first try. The section now says what is true, and says why it is the
  better property: the store is the one an organisation already maintains, so a CA it removes
  there stops being trusted here too, which a compiled-in bundle would not do. It also states
  the control that actually limits where bytes go — the register, not the certificate store.
  `cyberbrain policy egress` prints the same sentence, so the question can be answered from
  the program, and a test compares all three so they cannot drift apart again.

### Changed

- The hub's record migrates in place. `CREATE TABLE IF NOT EXISTS` does nothing to a table
  that already exists, so an upgraded hub would have kept the columns it was created with
  and failed on the first query naming a new one. Found by upgrading a hub that had been
  running for an afternoon; a record meant to hold a decade cannot start over on an upgrade.
- A test now fails when a message carries the indentation it was written with. `rustfmt`
  collapses a multi-line `format!` string onto one line and keeps the continuation's spaces,
  which is how `no egress gate is                  wired in` reached a shipped error message.
  It happened three more times in an afternoon, so it is checked rather than remembered.
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

[Unreleased]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.3.0...HEAD
