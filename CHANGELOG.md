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

- **`cyberbrain install`.** The hooks and the MCP server have been there since the first
  release; what had never been there was a way to switch them on. Turning them on meant
  finding another program's configuration file, knowing where that program keeps it, and
  adding an object to it by hand. That is not a setup step, it is the point at which a setup
  is abandoned.

  One command now does it: the six lifecycle hooks into this project's Claude Code settings,
  and an MCP entry into each desktop client found on the machine. Nothing that is not ours is
  changed, our entries are marked so `--undo` can take out ours and only ours, and the
  previous file is kept beside the new one. A configuration file that does not parse is
  reported and left exactly as it is, because a settings file with a trailing comma in it is
  somebody's unfinished work, not an empty file to be replaced with ours.

  In the Windows launcher it is a menu entry, "Set up Claude on this computer…", which is
  where it matters: somebody who installed this from a setup program has already said they
  would rather not open a prompt.

- **Every Claude Desktop installation gets the entry, not the documented one.** The Microsoft
  Store build is packaged as MSIX, and MSIX virtualises `%APPDATA%`: the application reads its
  configuration inside its own package folder, while the *Edit Config* button in that same
  application opens the unvirtualised path that every guide on the internet names. The two
  files are never synchronised, nothing warns, and a server configured in the documented place
  simply never loads. So presence is decided on the folder each installation creates, never on
  the configuration file, and every installation found is written. Doing it the documented way
  would have shipped a one-click setup whose one click does nothing.

- **ChatGPT is reported as unavailable, with the reason.** Its MCP support wants an HTTPS
  endpoint with OAuth, so a stdio server on the user's own machine cannot be registered there,
  and making one reachable from the internet is the opposite of what this product promises.
  Codex CLI is reported too, with the two lines to paste: its configuration is TOML somebody
  wrote by hand, with their comments in it, and reformatting that to save a paste is a bad
  trade.

- **Several projects open at once, in one launcher.** A project switch used to be a
  replacement: the running server was killed and the new one took its place, because
  everything from the settings file to the tray menu assumed exactly one. Opening a second
  project now adds it, and each keeps its own server on its own port.

  The tray icon holds a submenu per project, carrying that project's own actions — open, its
  folder, setting up Claude, connecting to a hub, closing it. Every one of those writes to
  one particular store, and a menu that leaves which one implied is a menu that sets up the
  wrong project. The names in it grow by an enclosing folder when they would collide, so
  `work/api` and `personal/api` never both read `api`.

  The single-instance rule stays. Two tray icons with no way to tell which held which
  project was never what anybody wanted; what changed is that the one icon now holds a list.
  A launcher started while another is running still opens the browser at what is already
  there, now at the project opened most recently.

  Everything that was open is opened again on the next start. A project that no longer
  starts is dropped and named, all of them in one message rather than a dialog each. A
  server that stops on its own now closes that project and leaves the others alone, where it
  used to take the whole launcher down with it.

  Anybody upgrading keeps the project they had open: the old single-project key is read
  once, folded into the list, and never written again.

- **A window of its own, instead of a browser tab.** The launcher opened the system browser,
  which made a program with a Start menu entry and a notification area icon feel like a
  bookmark. A project's page now opens in its own window, with its own taskbar button, drawn
  by WebView2 — a runtime component of Windows, not an engine we ship.

  It is a setting, not a replacement. Open in › A window of its own, or › The web browser;
  the browser is a real preference, with bookmarks and extensions and a window already full
  of tabs, and it is also the way back on a machine where WebView2 is missing. That case is
  handled rather than assumed: the launcher says what is wrong, opens the browser instead,
  and keeps doing so until it is restarted or the setting is changed back.

  Still no second frontend. The window is a frame around the page `serve` already answers
  with, at the same loopback address the browser would have been sent to; there is no
  navigation bar, no tab strip and no chrome of our own, because each would be an interface
  to maintain beside the one the product has. One window per project rather than tabs, for
  the same reason: a tab strip is something we would have to draw.

  The dependency was measured before it was chosen. `webview2-com` adds 13 crates to the
  lock file, all of them Microsoft's own windows-rs family; `wry`, the obvious alternative,
  adds 109 — an HTML and CSS parsing stack and bindings for Android and iOS, in the lock
  file of a launcher that runs on Windows and nowhere else, all of it read by cargo-deny.
  The price of the smaller graph is that the window is ours to write, and it is about three
  hundred lines.

  A browser engine is the hardest case there is for the promise in SPEC §12.1, so it is
  admitted on conditions and they are written down there: pointed at loopback and nothing
  else, started with background networking, component updates, sync and SmartScreen
  reputation lookups switched off, and confined to the launcher, which is not the store.

- **The command line, in the window.** The page could search, write, read the graph, run
  doctor and scan, and answer every compliance question — but `find`, `export` and anything
  else without a screen meant leaving for a terminal, and on the desktop that is a terminal
  somebody chose an installer to avoid. There is now a command line beside the other screens.
  You type the same thing you would type at a prompt, and you get back the same stdout,
  stderr and exit code.

  It is the CLI, not a copy of it. `POST /api/v1/command` splits the line — quotes and
  backslash escapes and nothing else, so there is no shell for a semicolon or a pipe to mean
  anything to — parses it with the same clap definition, and hands it to *this same binary*
  as arguments with `--store` fixed to the store being served. An in-process dispatch would
  have been a second place where `write` decides what a PII hold means and a second place to
  forget when a command grows an argument; SPEC §8 already refuses that trade for
  `--dry-run`. A held write comes back with exit code 3 and the operator's four choices,
  because it is the real thing that answered.

  The store is never ambiguous and cannot be argued with: a command typed in a window runs
  against that window's store, and `--store` is refused. What else is refused is an
  exhaustive match on the command enum, so a command added to the CLI stops the build until
  somebody decides whether it belongs in a window. `serve`, `mcp`, `hook`, `init`, `install`
  and `hub` are out, each for its own reason; so are `import` and `verify-export`, because
  both read files from anywhere on disk by name and this surface has no authentication —
  it has none because nothing it holds leaves the machine, and that sentence has to stay
  true.

- **Every project's command line in one window, side by side.** One command line per window
  meant window-hopping to fire something in one project while working in another. The tray
  now has "All command lines side by side": one window, one pane per open project, each pane
  that project's own page opened straight at its command line.

  Nothing reaches across the panes, and nothing needed to be relaxed for this. Each pane is a
  page at its own project's loopback address, so the browser's origin rule and the page's own
  `connect-src 'self'` keep them apart exactly as before; `--store` is still refused on the
  command endpoint. The window is ours, so several views in it cost a layout and no new
  surface. The panes tile as a grid rather than a row, because four command lines side by
  side on a laptop are four columns narrower than the lines they have to show, and the
  arithmetic is tested against awkward window sizes so no seam of unpainted window is left
  down the middle.

  The command line screen now names its project, from the store path it already knows.
  Four identical boxes next to each other is how somebody types into the wrong one.

- **`propose` and `review`: a note somebody else has to accept.** Rings 0 and 1 are the
  operator's, and until now that was a convention — the agent was told to propose text and
  never write it, with only the pre-tool-use hook refusing raw edits behind it. It is a
  mechanism now. `cyberbrain propose` writes into `proposals/`; somebody else runs
  `cyberbrain review <name> --accept`, and only then does it become a note.

  **A proposal is not in the index, and `recall` cannot return one.** That is the point, not
  a side effect: an agent that retrieves an unapproved ring 0 note treats it as an invariant
  nobody agreed to. It is also why a proposal lives outside the notes tree rather than
  carrying a state in its own header — `Frontmatter` does not deny unknown fields, so a
  state there would be read and ignored by every older binary, which is the failure with the
  state in it. A directory an older `scan` never walks cannot be ignored into existence.

  **A proposal cannot be reviewed by the person who made it**, in the same words the hub
  already uses for a disclosure request. Who proposed it comes from the audit log rather than
  the file: the chain is hashed, and a line of YAML in a file anybody can edit is not. A file
  that turns up in `proposals/` without a `note.proposed` row is listed and cannot be
  accepted — there is nobody to check it against.

  This is a workflow with a record, not an authentication: anyone who can write the identity
  file is anyone. It stops a mistake, not a determined person. The hub's version of the same
  rule is backed by tokens, and a deployment that needs enforcement rather than evidence
  belongs there.

  Identity comes from `CYBERBRAIN_IDENTITY`, then a line in the user's own configuration
  directory, then `git config user.email`. Deliberately never from `cyberbrain.toml`: that
  file travels with the repository, so a name in it is committed on behalf of whoever clones
  it next — and `Config` denies unknown fields, so a new section there would make every older
  binary refuse the store outright.

  The PII gate runs at both ends, at `propose` so the author answers for their own text and
  again at `accept` because the proposal may have sat for a week. Accepting a proposal for a
  note that changed in the meantime is refused unless forced. Rejecting needs a reason.
  Session-start says how many are waiting, since a proposal nobody is told about is a file in
  a folder.

- **A terminal in the window.** `serve --terminal`, and in the desktop launcher always: a
  real pseudo-console with a program of your choosing in it, in the project's directory. A
  shell, `ssh -o ServerAliveInterval=60 root@…`, an agent — whatever you would run at a
  prompt. xterm.js draws it, a WebSocket carries the bytes, and ConPTY on Windows or a pty on
  Unix runs it. Both platforms, because a terminal whose only implementation runs where
  nobody here can try it is a terminal nobody has tried.

  **This is the change that made Cyberbrain a workbench and not only a memory, and two
  promises had to be rewritten to stay true.**

  SPEC §8.1 said there was no authentication because there was no remote access to
  authenticate. That was about *remote*, and it held while the worst a local caller could do
  was write a note. A shell is a different thing, so the terminal does not inherit the
  exemption: it is off unless asked for, it needs a token minted per run and handed to the
  page in the URL fragment (which a browser never puts in a request, so it reaches no log),
  and it refuses a handshake carrying an origin that is not ours — a WebSocket handshake is
  not subject to the same-origin rule, so without that check any page you had open could try
  for a shell. A handshake with no origin is admitted, deliberately: that is a program on
  this machine running as you, which can start a shell without our help.

  SPEC §12.1's egress register can no longer claim to enumerate every path bytes may take,
  and pretending otherwise while `ssh` is one keystroke away would make it false — a false
  register is worse than an honest gap. So the terminal is *in* the register, as the one
  entry the gate does not mediate, saying so in words. The promise that survives is narrower
  and is the one the product is sold on: **a note never leaves this machine by any path
  Cyberbrain takes.** What you do in a terminal is yours, in your name.

  Measured before it was chosen: ConPTY costs no new crate at all (feature flags on
  `windows-sys`, already here), the WebSocket costs five, and xterm.js is loaded only when
  somebody opens a terminal — the main bundle is the size it was.

- **Saved command lines.** The servers you connect to and the tools you start, as buttons:
  type a line, press Save, name it. They live in `terminals.toml` in your own configuration
  directory, never in the store — the store travels with the repository, so a host name and
  an account name in it would be committed on behalf of whoever clones it next. They are
  behind the same token as the terminal, because the list says which machines this person
  reaches and under which account.

  A saved line is split by the same code that splits a line you type, so the two cannot
  behave differently. The page had a splitter of its own for a day; it is gone, and the
  contract lives once, on the server.


### Fixed

- **A decided proposal kept vouching for its own name.** `review` asked the audit log whether
  a `note.proposed` row existed for a name, and those rows never leave the log because the
  chain is hashed — so a proposal that had been rejected months ago still answered. Anyone
  could put a file of that name back into `proposals/` with any content and any ring, and
  `--accept` would find the old row, apply the two-person rule against somebody who had
  nothing to do with it, and write an unapproved ring 0 note whose audit trail then named
  that person as its proposer. The state of a name is now the newest of proposed, accepted
  and rejected, and only `proposed` is an open proposal.
- **A typed command could write a file wherever it was pointed.** `POST /command` let
  `policy audit --export <path>` through, and that route needs no authentication, so any
  local process could place or destroy a file as the account running `serve` — including this
  store's own audit log. The refusal list had considered reading and stopped there.
- **A paste could hang the whole server.** The write half of a terminal blocked the single
  runtime worker as soon as the program inside stopped reading its input.
- **Every Windows path typed into a terminal was broken.** The line splitter treated a
  backslash as an escape, which is a Unix shell's rule and the wrong one here — the desktop
  launcher always starts a terminal, so Windows is the platform this lives on, and the first
  thing anybody types into one is a path. `C:\Users\me\tool.exe` became
  `C:Usersmetool.exe`, and a saved connection passed its own validation and then never
  started. A backslash stands for itself now; `\"`, `\'` and `\\` are the only escapes,
  which is all that is needed to put a quote inside a quoted argument.

  The other half of the same mistake, invisible until this one was fixed: joining argv back
  into a Windows command line doubled every backslash, where `CommandLineToArgvW` only treats
  them as special in a run immediately before a quote. That code moved out of the Windows-only
  module so it is tested on every platform — the rule is string handling, the mistake was
  string handling, and a rule that can only be checked where nobody can run the checks is a
  rule nobody checks.
- **Switching the language killed every open terminal.** The effect that owns the socket had
  the translation dictionary in its dependency list — for the sake of one label — so changing
  the language tore the socket down and the server killed the process behind it. One
  keystroke and a running `ssh` session was gone, without a word.

### Added

- **Browser tests.** `npm run e2e` in `ui/` starts a real `cyberbrain serve --terminal` over
  a throwaway store and drives the page with Playwright. The page has parts only a browser
  can be wrong about — a WebSocket the content security policy has to allow, a token that
  arrives in the URL fragment, an effect whose dependency list decides whether switching the
  language destroys somebody's session — and every one of those has been wrong at least once.
  None of it is visible to `tsc`.
- A flake in the launcher's tests, seen roughly one run in ten and misdiagnosed once before
  it was found. The fakes those tests use are shell scripts they write and then execute, and
  on Linux a program cannot be executed while any descriptor to it is open for writing —
  with tests in parallel, one thread's `fork` inherits another thread's write handle and the
  spawn fails with `ETXTBSY` on a file that is perfectly fine. It failed on whatever change
  happened to be under test, which is the worst property a test can have.
- The command surface in SPEC §8 had not listed `hub` or `verify-export` since they shipped.
- The launcher's WebView2 profile is kept in `%LOCALAPPDATA%\cyberbrain\WebView2`. The
  default is a folder beside the executable, and after an install that executable is under
  `Program Files`, which the user cannot write to — so every window would have failed to
  open on exactly the machines the installer is made for, and on none where it was tried
  from a build directory.

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
