# Changelog

What changed, in the words of someone who has to explain it to a user rather than to a
compiler. Notable changes only; the commit history has the rest, and it is written to be read.

This file is also the record of when each version shipped, which the licence needs: under
[FSL-1.1-ALv2](LICENSE.md) every release turns Apache-2.0 two years after **its own** release
date, so that date has to survive somewhere more durable than a tag that can be moved.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow
[semantic versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Windows launcher, installer and web page

- **A new mark: the neon brain with its orbit ring**, in the tray, on the installer and its
  shortcuts, in Apps and now also in the window's title bar and taskbar button, which showed
  Windows' blank program icon before.
- **The page no longer says notes never leave the machine.** The hub line in the sidebar and
  its tooltip now say what is true since note sharing: by default only the audit trail goes,
  the text of a note only when `allow_note_sync` is on, the note has a bereich and that bereich
  is granted to this machine, and rings 0 and 1 never. The compliance headline says "nothing
  went to a public destination" instead of "nothing left this machine", because traffic to the
  hub and to your own network is counted separately below it.
- **The launcher speaks German on a German Windows**, in the tray menu, its dialogs and the
  WebView2 message, and so does the installer (pages, messages, section descriptions); every
  other language gets English.
- **The installer's "Open Cyberbrain" box starts the launcher as the signed-in user.** It used
  to start it under the installer's elevated token. It now goes through `explorer.exe`, the
  stock NSIS way, with no extra plugin.
- **The hub delivery schedule counts seconds.** The message pump called the tick after every
  window message, so with a window open or the mouse over the tray icon the "every 15 minutes"
  push ran every few seconds.
- **Closing a project from the menu also closes the side-by-side window** that held its
  address. A second start asks the running launcher to show its page (window or browser, as set,
  with the terminal) instead of opening a bare browser tab; an older launcher still gets the
  tab. The side-by-side panes show the command line without the sidebar.
- **The launcher stays responsive while a project starts**, while the clients are set up and
  while an invitation is enrolled; those run on threads, the tooltip says what is starting,
  projects reopened at login start in parallel, and `init`, `install` and `hub enrol` give up
  after 60 seconds instead of holding the launcher for good.
- **New: "Start with Windows"** in the tray menu (per user, `HKCU\...\Run`, off by default,
  starts with `--background`: no window, no folder question).
- **Dialogs trap focus, close on Escape and fit the window.** The PII question, the forget
  confirmation and the shortcut list share one shell; the page behind them hears no shortcuts
  while one is open, and their buttons stay in view above a scrolling body.
- **"In the team" shows "10 of 37" with a Show all button** instead of stopping at ten under a
  count of 37; the team screen and the sidebar say "checking" or "unavailable" for the hub
  instead of "not enrolled" while the answer is missing.
- **Errors lead with a sentence**; the code, the status and the server's text are under
  "Technical details". Console output is bounded in height and scrolls.
- **New: search "as of" a day** (`recall --stand`), with superseded, no-longer-valid and
  not-yet-valid badges; the HTTP client had been dropping `stand`. The simple view no longer
  reads out a conflict's loser or a replaced note as the answer, and shows the caveats.
- **New: approvals show a line diff** against the current note when a proposal changes one.
- **Block links scroll to the block** they name (matched by its preview), in both views, and
  flash it. A tag in a note filters the list; a broken `%` in the address no longer blanks the
  page; a slow audit answer for an old filter no longer overwrites the new one; an audit export
  that hits the page limit is named `-partial` and says so; a cleared search box stays clear.
- **The graph works from the keyboard and by touch** (arrows pan, `n`/`p` walk the nodes, a
  node list, pointer events). Kinds, PII kinds and destination classes are German in the German
  page; `note.proposed` has its own audit filter. A title typed in the simple view is kept as
  the note's first heading; Back from a note returns to the question.
- Tests: `npm test` (vitest, node) runs in the `ui` job.

### Added
- **Hub alerts to a webhook (`cyberbrain hub alerts set|show|test|off`).** A hub can post what
  needs a person (countersignatures waiting, the licence ending or gone, devices quiet for 48 h,
  governance calls failing) to one webhook its administrator sets. Off by default; nothing
  leaves until a URL is set, and then only there, through the new egress path `hub-alert`
  (loopback or private range unless `--allow-public`, no redirects). It carries states and
  device names, never notes or activity rows. Posted when a condition appears, reminded daily,
  every attempt in the hub's own log. The egress register now has nine entries; a store lists
  `hub-alert` as a path only a hub uses.
- **People and fleet invitations on the hub's page.** The administrator's page lists the people
  with a role and the fleet invitations, and does what `hub principal` and `hub invite` do: add
  an auditor or an editor (credential shown once), withdraw a person, give an editor a bereich
  or take it away (new), make a fleet invitation (written owner-only to `invitations/`, with the
  certificate pin) or withdraw one. Countersigners and administrators are still made only with
  the command at the machine, so the people who sign are not made by the person whose acts they
  sign. New routes, all POST, administrator only: `/principals`, `/principals/{id}/revoke`,
  `/principals/{id}/bereiche`, `/principals/{id}/bereiche/{bereich}/remove`, `/enrolment-codes`,
  `/enrolment-codes/{id}/revoke`.
- **A store can sign its audit log.** The hash chain showed that a row changed, but anybody
  who could write `audit.db` could recompute every hash after an edit, and an export proved
  only that it was self-consistent. `cyberbrain policy audit-key --init` gives the store an
  Ed25519 key (`keys/audit.ed25519`, owner-only) and prints its fingerprint. From then on every
  new row is signed, `policy audit --export FILE --signed` writes a signed bundle, and
  `cyberbrain verify-export FILE --expect-key FINGERPRINT` checks the bundle and every signed
  row in it against the key the recipient was given. `status` shows the fingerprint,
  `policy audit --verify` counts signed rows and those written before the key. Without a key
  nothing changes.
- **`forget --report FILE [--sign]` writes an erasure report somebody else can check:** what
  was removed, whether the WAL was emptied, and the chain hashes of the two audit rows that
  record it, signed with the store's key when asked. `verify-export` checks it.
- **Decoys (`cyberbrain canary plant|list|remove|push`).** A store can plant a credential nobody
  uses (an API key, an IBAN with a valid check sum and no bank behind it, a password, a token)
  in a note that reads like the real thing and is tagged `canary` for people. A use of it
  anywhere but its own note is refused: in any tool call the pre-tool-use hook sees, before the
  governance service is asked, and in any note written or proposed. Each use is counted and
  recorded as `canary.hit`, and a hub that receives the row raises an alert. The value is never
  in the audit log, a refusal, an alert or an answer of `serve`; they carry its fingerprint (12
  hex digits of SHA-256). `push` hands the decoys to AgentGuard (`POST /v1/koeder`, key scope
  `koeder:write`). The value lives in `keys/canaries.json`, owner-only and kept out of git.
- **A "Decoys" card on the status page.** How many decoys are planted, how often one was used
  and when last, from the new `GET /api/v1/canaries` (counts, labels and fingerprints; never a
  value). A used decoy turns the card red.
- **MCP client tokens can expire (`mcp-client add <name> --expires
  P90D|2027-03-31|<timestamp>`).** An expired token is refused at `mcp --http` with a 401 that
  names the client and the moment it ran out, and the refusal is in the store's audit log
  (`policy.refusal`, subject `mcp-client:<name>`). `mcp-client list` shows the end date. A token
  without one works as before, and a file without expiries is read and written exactly as
  before.
- **Hub credentials and device tokens can expire (`hub principal add --expires`, `hub add
  --expires`, and a field on the page).** After its end a credential is refused by name, at the
  command line and at sign-in, and sessions opened with it end; a device is refused on every
  delivery and fetch. The hub's log records `credential.expired`, `device.token_expired` (once
  per device) and `login.refused` with `"credential": "expired"`. `principal list`, the fleet
  view and the page show the end, and hub alerts gain `token-expiring` (within 7 days) and
  `token-expired`.

### Fixed

- **A note pushed late reaches every device again.** The sync cursor was the newest `updated`
  in the answer, which is the writer's clock, and the hub filtered on it. A note written
  offline at 12:00 and pushed at 13:00 sat below the cursor of a device that had pulled at
  12:30 and never arrived there. The hub now numbers what it takes in (notes, erasures, a
  conflict resolved for the offered version), and the cursor is that number (`#42`). A cursor
  from an older hub is a timestamp, which is no longer read: the first pull after the upgrade
  gets everything once, which the device already has or takes again. An older hub handed the
  new cursor sends everything rather than nothing.
- **The same `updated` with other text is a conflict, not a re-send.** The hub compared only
  the stamp, so a note edited by hand without touching `updated` was taken for the version it
  already held and dropped without a word. It now compares frontmatter and body as well; a
  true re-send is still quiet, and anything else is kept as a conflict like any other.
- **The governance fallback no longer waves writing commands through as reads.** With
  AgentGuard unreachable in enforce mode, a Bash call went ahead without asking if it looked
  like reading, and several did not only read: `` echo `rm -rf x` `` (backticks were not
  split), `sort -o f`, `uniq in out`, `find -fprint f`, `git branch -D x`, `git diff
  --output=f`, `sed -n 'w f'`. The check is now fail-closed: command substitution is refused
  outright, `sort`, `uniq`, `git branch` and `sed` pass only with flags and scripts it knows to
  read, `find` and `git log/show/diff` are refused with any writing option, as are `tree -o`,
  `date -s` and `rg --pre`. What it cannot take apart asks, as every other call does then.
- **A secret that contains an email address is redacted whole.** Of two overlapping
  findings only the higher-priority one was kept, so in `password = "alice@corp.de!S3cret9"`
  the email pushed the secret aside, redaction left `[redacted:email]!S3cret9`, the rescan
  found nothing in that, and the note was marked Reviewed with the rest of the password in
  it. Overlapping findings are now merged into one over the whole stretch, named for the
  finding that spans all of it (here the secret).
- **Invitations and device tokens are owner-only from the moment they exist.** The
  invitation the hub page writes (it carries the device token in the clear) and the one `hub
  add --invite` writes were created with the default mode, on most systems readable by
  everybody on the machine. The client's token file was written first and set to 0600
  afterwards, so it was readable for a moment, and a reader that had opened it earlier kept
  reading through the change. All of them, and the `mcp-clients` list, are now written into a
  new 0600 file that is renamed over the old one (Unix; on Windows the file keeps the
  directory's permissions, as before).
- **The hub's own audit chain no longer forks when the CLI and the server write at once.**
  Reading the last hash and appending after it were two steps, and the server's lock does not
  reach a `cyberbrain hub …` command that opens `hub.db` itself; two rows could continue the
  same row, and the append-only triggers keep such a fork for good. Appending now happens in
  one `BEGIN IMMEDIATE` transaction, which SQLite holds across processes, a second writer waits
  up to 10 s instead of failing with "database is locked", and a unique index allows one
  successor per row. A hub that already forked still opens (the index is left out there) and
  `hub verify` names the fork: which row continues which, a second time. Nothing is repaired.
- **A flood of sign-in attempts no longer stalls the hub.** The password was checked with
  argon2 while the hub's one lock on its record was held, so every guess somebody on the
  network sent held up every device's push and every page for the length of the check. The
  record is now held only to read the stored hash; checking it, and hashing a new password on
  the change form, run outside it, on a blocking thread, at most two at a time.
- **A new hub password signs out the other administrator sessions.** Changing it left every
  open session alone, and each use pushed its expiry out again, so a cookie left open on
  another desk stayed good until the hub restarted. Every administrator session but the one
  that made the change now ends with it; that one has just proved the current password.
  Principals' sessions stay, since their credential did not change.
- **"Sign out" on the conflicts and requests pages signs out.** Those pages sign out with a POST
  form, and `/logout` took GET only, so the button answered 405 and the session stayed open,
  which on a shared machine is the case the button exists for. `/logout` now takes both.
- **A disclosure that cannot be recorded no longer happens.** `hub disclose` wrote the rows
  first and recorded the disclosure afterwards, discarding any error, so a reading could take
  place without a trace in the hub's chain. The rows are now gathered, the disclosure recorded
  (count and chain row in one transaction), and only then written; if the record fails, nothing
  is written. A request also has to name a device the hub knows: the id becomes a file name, and
  `../x` was written outside the chosen directory.
- **An invitation made on the hub page pins the hub's certificate.** `hub add --invite` and
  fleet invitations carried the fingerprint, the page wrote a version 1 file without it, so a
  hub serving its own certificate could not be reached from a machine invited through the page.
  The page now writes version 2 with the pin, which clients since 0.4.0 read.
- **Deciding a hub conflict no longer overwrites a newer version, and takes all of the offered
  one.** Taking the offered version replaced whatever the hub held at that moment, also a
  version that arrived after the conflict was raised, in two separate statements; ring and kind
  stayed those of the held version. It is now one transaction that first checks the held version
  is still the one the conflict was about (otherwise it refuses and the conflict stays open,
  also on the page, which says why), and takes the offered ring and kind as well. Conflicts
  raised before this version keep the held ring and kind, since they never recorded the offered
  ones.
- **A bereich granted later arrives whole.** A device's sync cursor could already be past the
  notes of a bereich it was granted afterwards, so the new grant brought only what changed from
  then on. The hub now notes where its count stood when a grant took effect (the
  countersignature), and a fetch delivers that bereich from the start, notes and erasures, until
  the device's cursor has passed that point: once. Grants that took effect before this version
  are left as they are.
- **Device tokens, invitations and the MCP client list get their own ACL on Windows.** They were
  owner-only on Unix and inherited the folder's permissions on Windows, which under
  `C:\ProgramData` include read for `Users`. They are now created with a DACL that inherits
  nothing and grants the local system account, the administrators group and the file's owner, as
  the hub's private key already was (that one without the owner).
- **`forget` says when the erased text may still be on disk.** The scrub after an erasure ran
  `wal_checkpoint(TRUNCATE)` and threw the answer away. With `serve`, `mcp` or the daemon
  holding a read transaction on the index, the checkpoint cannot reset the WAL, the
  pre-erasure pages stay in `cyberbrain.db-wal`, and `forget` still reported a clean erasure
  (B5). The report now carries `scrub.residue` and SQLite's own numbers
  (`scrub.wal.busy`, `log_frames`, `checkpointed_frames`), and says in words when pages may
  remain and how to get rid of them. `policy retention --apply` reports the same.
- **An audit export narrowed by `--action` or `--subject` verifies.** The file was written,
  and then its own check refused it, because the rows of one action are not neighbours in the
  chain (K10). Such a file is now format version 2, names its selection in the header, and is
  checked row by row: every row is as it was written and belongs to the selection. It cannot
  show that the selection is complete, and the output says so. A plain period is still
  version 1, byte for byte. A hub refuses a selection as a delivery.
- **Signed audit rows hash every field with its length.** The original rule separated actor,
  action and subject with a newline, and a field may contain one, so moving a boundary kept
  the hash (K1). Rows signed with a store key use length-prefixed fields (chain format 2,
  `_chain.v = 2`); every row written before keeps its own rule and verifies as before.
- **A note that is a symbolic link is no longer followed.** The walker stayed out of linked
  directories but listed a linked `.md` file inside a ring and read wherever it pointed, so
  write access to the store was enough to have any readable file indexed. Such a link is now
  reported as skipped by `list` and refused by every note and proposal read.
- **`inference.allow_overlay_network` works.** The switch was honoured by the address check
  and the egress gate, but loading the config refused a `100.64.0.0/10` endpoint as public
  before either was reached, so a Tailscale GPU box needed `allow_public_endpoint`, the very
  consent the switch was made to avoid. The config now accepts a 100.64/10 address literal
  when the switch is set, and nothing else; without it the refusal names the switch.
- **An Ollama digest with the `sha256:` prefix names the same profile as without.** The
  profile id took the first twelve characters as written, so the prefix used up seven of
  them, five hex digits were left to tell models apart, and changing only the spelling
  rebuilt the index. The digest is now read without the prefix and in lower case.
- **A note that cannot be read back is no longer written.** Writing a note checked its name
  and validity window, reading it also the `bereich`, `retention` and supersession names.
  A client that skipped those checks (MCP `write` did not check the `bereich`) could store a
  note the next read refused. Writing and reading now apply the same checks.
- **Headings that fill the block limit are a block of their own.** A run of headings with
  nothing between them took the first words of the next paragraph along, went over the limit
  and was reported as "a single run of characters longer than the limit", which it was not.
  They now stand alone, and when they alone exceed the limit the report says so. Only notes
  with such a run of headings are split differently, and their blocks are re-embedded once.
- **A refused or failed request no longer writes credentials or a query into the audit.**
  A URL with `user:password@` was refused, but the refusal reason quoted it whole and the
  `policy.refusal` row carried it, which audit sync can send to a hub; a failed download or
  hub request quoted its full URL, query included, in the row that closes it. Both now show
  the address without user name, password, query or fragment, and `user@host:port` is
  refused as credentials rather than looked up as a host name.
- **A slow or oversized answer can no longer hold a request open or fill memory.** The
  transport for model downloads, hub sync, enrolment and governance had a connect and a read
  timeout, but the read timeout restarts with every byte, and any body was read whole. Each
  request now has an overall deadline (one hour for a model download, ten minutes otherwise)
  and a body limit (2 GiB for a model artefact, 256 MiB otherwise).
- **A statutory deadline can be written out as JSON.** `Deadline::Hours(72)`, the EU breach
  deadline, could not be serialised at all, so neither could the breach rule holding it.
  Nothing renders one yet; the next thing that does would have failed. A deadline is now
  `{"kind": "hours", "value": 72}`, and one without a number is `{"kind": "one-month"}` as
  before.
- **`*.localhost` is loopback for the egress gate too.** The config accepted
  `http://gpu.localhost` as local, while the gate asked the system resolver and refused it on
  any machine where that had no answer. The gate now answers names under `.localhost` with
  the loopback addresses itself (RFC 6761), and the connection is pinned to them.
- **A client limited to some bereiche no longer sees notes outside them through recall.**
  The scope was checked once at the start of a recall; a note moved into a foreign bereich
  while the recall ran was still returned. And `superseded_by` named the replacing note
  even when it lay outside the scope, which `newer_links` already avoided. The scope is now
  checked again when hits are read, and a successor outside it is not named: the hit is
  still marked down as replaced, and a caveat says that its successor is not shown.
- **Embedding load options are part of the profile.** `max_tokens` and `unk_token` change the
  vectors a static model produces but did not change its profile id, so vectors made with
  different settings could have been mixed in one index. They now count when set. Loaded
  with the defaults, as the binary does, the id is unchanged, and existing indexes stay valid.
- **A trailing dot in the inference host no longer bypasses the address pin.** The checked
  addresses were pinned for `gpu.lan`, but requests went to `gpu.lan.`, which the pin did
  not cover, so the name was looked up again when connecting. Requests now go to the host
  without the dot, to the addresses that were checked.
- **An IPv6 zone in the inference URL is refused with a reason that says so.**
  `http://[fe80::1%25eth0]:11434/v1` was rejected as "not a private range" or as an
  "invalid IPv6 address". Zones are still not supported, and the refusal now says that and
  suggests a unique-local address or a host name instead.
- **An inference or embedding endpoint can no longer fill memory with its answer.**
  Completions, model lists and embedding answers were read whole, and a stream buffered an
  event for as long as it lasted. Answers are now read up to 16 MiB, a stream event and a
  streamed text are held to the same limit, and error bodies to 64 KiB. A larger answer is
  refused and recorded as a bad response.
- **A streamed answer that breaks off is reported, not taken as complete.** A stream that
  closed without `[DONE]` was recorded as a successful call, and its last event was dropped
  if no blank line followed it. The last event is now read, and a stream that ends with
  neither `[DONE]` nor a finish reason is an error and is recorded as a bad response.
- **A model reply with braces in its prose is still understood.** Only the first `{` was
  read as the answer, so "given {x}, here it is: {...}" failed on `{x}`. Each object in the
  reply is now tried in turn; one nested inside a wrong answer is never taken for it.
- **`find` no longer lists definitions that are only text in a docstring or comment.** A
  Python docstring with `def fake():` in it, or a commented-out function between `/* */` in
  JavaScript, Go or Rust, showed up as a symbol. Lines inside a multi-line string or comment
  are now skipped.
- **The Rust character literal `'\''` is read whole.** The code lexer ended it at the escaped
  quote and read the real closing quote as a lifetime. It now closes where it should.
- **A TypeScript function with an object return type is found whole.** In
  `function f(): { a: number } {` the return type was taken for the body, and `find` returned
  the first line alone. The body is now the brace after the return type.
- **A Rust impl with a closure bound is named for its type.** In
  `impl<F: Fn() -> U, U> Holder<F>` the arrow ended the generics, and `find` listed the impl
  as `U`. It is now `Holder`.
- **`.cyberbrainignore` wins over `.gitignore` wherever the two sit.** A whitelist such as
  `!x.py` in a subdirectory's `.gitignore` brought back a file the root `.cyberbrainignore`
  excluded. The `.cyberbrainignore` files are now consulted first, then the `.gitignore`
  files, as documented.
- **A damaged line in `usage.jsonl` no longer takes the usage view down.** The date was cut
  from the timestamp at byte 10, which panicked when that byte fell inside a multi-byte
  character. Such a line is now skipped like any other without a date.
- **The approval lookup encodes what it puts into the URL.** `approval_status` placed the
  action id and the tenant into the governance service's URL as they were, so a `/`, `?`, `&`
  or `#` in either chose another route or added a parameter. Both are now percent-encoded.
- **The store guard sees `sed -ni`, `perl -pi` and `sed --in-place`.** A shell edit counted as
  in-place only when an argument began with `-i`, so the same option bundled with others
  changed ring 0/1 notes, waiting proposals and the audit log past the guard. Short option
  clusters are now read letter by letter, and `--in-place` counts.
- **The store guard stays closed when the audit log cannot be written.** A refusal was
  recorded before it was given, and a failed append turned the whole hook into "no
  decision", which the harness allows. With a full disk or a locked `audit.db` the guard on
  rings 0/1, proposals and the audit log was open. The deny now stands and the missing row
  is reported on stderr.
- **A resident note is attested and injected from the same read.** The session hooks
  fingerprinted a ring 0/1 file and then read it a second time for the text, so a file
  swapped in between passed the operator's attestation with somebody else's text. Both now
  come from one read.
- **Duplicate hook runs that find an old claim no longer both go ahead.** When the same
  payload had been claimed more than two seconds earlier, every duplicate removed and
  recreated the claim and ran. Taking over an old claim is now itself an exclusive claim, so
  exactly one of them runs.
- **Windows: `\x` and `C:x` are placed where Windows places them.** The store guard joined
  every non-absolute path onto the working directory, so a rooted path (`\proj\.cyberbrain\…`)
  or a drive-relative one (`C:..\.cyberbrain\…`) aimed at the store was classified as
  somewhere else. Rooted paths now resolve on the working directory's drive or share, and
  drive-relative ones against the working directory when it is on that drive.
- **`propose-session` keeps the end of an oversized last message.** When the newest message
  alone was larger than the size cap, the extract stopped before it and proposed nothing
  but "[earlier part of the session omitted]". It now keeps the end of that message.
- **`import --suggest` no longer puts every subfolder into the top-level group.** The
  top-level group was proposed as `*.md`, which matches a file name at any depth, and it is
  checked first, so `HR/personen.md` was imported with the wrong kind and without its
  bereich. Plan patterns may now start with `/` to anchor at the root, and the suggestion
  uses `/*.md`. Plans written by an earlier `--suggest` still carry `*.md`: change the
  top-level group's paths to `"/*.md", "/*.markdown"` before running them again.
- **`import` no longer reads through symbolic links.** A link in the source tree was taken
  for a file and read, so a link in a foreign export (a wiki dump, a vault) imported
  whatever it pointed to. Links are now listed and reported as files not imported.
- **Two source trees with the same layout no longer overwrite each other's notes.** An
  imported note was tied to its file only by the path relative to the source root, so
  `existing = "overwrite"` on a second tree replaced notes the first had written. Imported
  notes now also carry a `src-root:` tag (a hash of the absolute source path).
- **Re-importing with a new retention or bereich applies it.** An item whose text and kind
  were the same counted as "already imported", so a retention period added to the plan to
  meet an erasure duty was never written, and the run reported success. A retention or
  bereich the plan sets is now compared as well, and a difference is an update.
- **`collisions = "hash"` works for long names.** The content hash went on the end of the
  name and the length limit cut it straight off again, so long colliding headings still
  collided and were not imported. The name is now shortened instead of the hash.
- **`install` no longer replaces or removes somebody else's MCP server named `cyberbrain`.**
  The Claude Desktop entry has no marker, so the name alone decided, and `--undo` deleted a
  foreign entry of that name. An entry now counts as ours only when it starts cyberbrain's
  MCP server; anything else is left as it is, reported as `not-ours`, with a hint to choose
  another name with `--name`.
- **Closing a terminal ends the background jobs started in it.** On Unix only the shell's
  process group was killed, and a shell with job control gives every background job a group
  of its own, so those kept running and held the terminal open. Every process of the
  terminal's session is now ended (Linux and macOS). On Windows only the shell was
  terminated; the shell now runs in a job object, and closing the terminal ends the job.
- **Answering a PII hold on a new note no longer overwrites a note created meanwhile.**
  Creating a note over HTTP refuses an existing name, but a creation held for PII was
  written later without that check, so a note of the same name made in the meantime (by an
  agent over MCP, in another tab) was replaced. The answer to the hold is now refused with
  409 `write-conflict` instead.
- **On Linux, `cyberbrain serve` answers only its own account.** The API asks for no
  credentials because it binds loopback, but loopback is shared by every account on the
  machine, so on a shared server any user could read, change and erase the store (and run
  commands) through `/api/v1`. Each connection's owner is now looked up in the kernel's
  socket table, and a connection from another account (root excepted) is dropped before it
  is read. The page, the desktop app and scripts run as the same user are unaffected. On
  macOS and Windows the API is still open to every local account; the startup line says
  which applies.
- **Values from an invitation are written into the store's config escaped.** Enrolment pasted
  the hub address and device id between quotes, so a quote and a line break in an invitation
  wrote configuration keys of their own. They are now TOML strings.

### Upgrading

- Nothing changes until `policy audit-key --init` is run. Existing `audit.db` files need no
  migration: the signature lives in the row's `detail`, and rows from before the key verify
  as "unsigned (written before the store had an audit key)".
- **Once a store signs, every binary that touches it must be this version or newer**, and so
  must a hub it delivers to. An older binary reports the first signed row as edited, and
  an older binary still writing (a daemon, `serve`, `mcp` that was not restarted) appends
  unsigned rows that the new check reports as a break. Restart them after `--init`.
- Signed bundles, selections (`--action`/`--subject`) and bundles holding signed rows are
  format version 2. A plain unsigned period is still version 1. `scripts/verify-audit-export.py`
  reads both; it needs `pip install cryptography` for signed files.
- A store whose `embedding.ollama.digest` is written with `sha256:` or in upper case gets a
  new profile id once, and its vectors are rebuilt on the next scan. A digest written as 64
  lower-case hex digits, as `ollama show --modelfile` prints it, keeps its id.
- Notes imported before this release have no `src-root:` tag and are still matched by
  `src:` alone; the tag is added the next time such a note is updated. A source tree that
  moves to another path is a different tree from then on: an `overwrite` import from the new
  place reports its notes as existing instead of replacing them.
- `import --suggest` plans written before this release use `*.md` for the top-level group,
  which still matches files in every subfolder. Change it to `"/*.md", "/*.markdown"`.
- `cyberbrain install` now reports `not-ours` for an MCP entry of the chosen name that does
  not start cyberbrain, and leaves it alone; use `--name` for a second entry.
- The hub record gains columns and a table on first start (`sync_seq`, `sync_clock`,
  `effective_seq`, `offered_ring`/`offered_kind`, a unique index on the hub log). All additive;
  an older hub binary still opens the record, and a hub whose log already forked opens without
  the index and `hub verify` names the fork. Clients need no update: the sync cursor is opaque
  to them, and the first pull after the hub upgrade delivers everything once.
- The hub record gains `expires_at` on `devices` and `principals` (additive, NULL = no end).
  Every credential and device token issued before keeps working as it did.

## [0.7.10] — 2026-10-23

- **MCP 2026-07-28.** The server is now dual-era: a request that names its protocol version in
  `_meta` is served statelessly (`server/discover`, `resultType`, `ttlMs`/`cacheScope` on the
  tool list, `serverInfo` in `_meta`, `UnsupportedProtocolVersionError` for a version it does not
  speak), and `initialize` also accepts 2025-11-25. Claude Code 2.1.292 and later probe
  `server/discover` first; before this release they fell back to `initialize` with 2025-11-25,
  which the server answered with a "protocol note" in the instructions of every session.
- **Session start in two hooks.** Claude Code caps each hook's output at 10,000 characters on its
  own and shows the model only a 2,000 character preview beyond that. `session-rings` prints
  rings 0 and 1 (and keeps the session state and the audit row), `session-context` the handoff,
  the digest and how to use the store, so the rings get a whole cap to themselves. `session-start`
  still prints everything, for installs that call it. `cyberbrain install` writes the pair, and
  `doctor` measures each half when the hooks are split, the whole text when they are not.
- **A timeout on every hook** `cyberbrain install` writes (30 s). Without one Claude Code waits
  600 s, so a hook stuck on a lock would have held every edit for ten minutes.
- **`cyberbrain propose-session --transcript <path> | --latest`**: a Claude Code session,
  summarised by the local model, filed as a proposal (ring 3, kind session) with its decisions and
  open items. Nothing enters the store until somebody accepts it with `review`; the PII gate runs
  as for any proposal. Only what was said is summarised: tool output and system reminders,
  including the rings this store injected, are left out, and a long session keeps its end.
- **llama.cpp and vLLM are recognised** on the status screen (`Server: llama.cpp`,
  `owned_by` `llamacpp` / `vllm`). NVIDIA PAIR 1.0 passes the engine's own answer through and
  sets no header of its own, so behind it the engine behind it is what is shown; said so.
- **Dependencies.** `cargo update`: rustls-webpki 0.103.16 (RUSTSEC-2026-0382), webpki-root-certs
  1.0.11 (RUSTSEC-2026-0383), ctutils 0.4.5 (RUSTSEC-2026-0385), tokio 1.54. `cargo deny` is clean.

## [0.7.9] — 2026-10-17

- **`cyberbrain doctor` checks the size of the session-start text.** Claude Code moves a
  SessionStart hook's output into a file when it is longer than 10,000 characters, and the
  model then sees only a ~2,000 character preview, so rings 0 and 1 are no longer in view.
  `doctor` builds the text the hook prints for a fresh start (no session is created, nothing
  is written), counts characters, warns above 8,000 and reports an error above 10,000, with
  the advice to shorten ring 0/1 notes and the handoff note.
- **Embeddings from a local Ollama** (`[embedding] backend = "ollama"`). A transformer
  embedding model instead of static token vectors: on 34 questions over this project's own
  notes the right note reached the top three for 88 % of them with
  `embeddinggemma:300m-qat-q4_0`, against 79 % with potion-multilingual-128M (hybrid; first
  place 71 % against 56 %). Documents and
  queries get the model's own prompts. The weights digest is pinned in the configuration and
  compared with what Ollama loads (`/api/show`) before embedding; other weights are refused,
  never mixed into the index. Every request goes through the egress register and the audit
  log. When Ollama is down, recall is lexical and says so, and a scan indexes notes without
  vectors and fills them in on the next run.
- **`cyberbrain bench-embedding`**: which embedding model fits this machine. Measures query
  latency and full-rebuild time of every installed candidate on a sample of the store's own
  blocks, and prints the configuration for the best one that fits. Downloads nothing.
- **A failed embedding no longer drops a note from the index.** It is indexed lexically and
  revectorised by the next scan. Before, a changed note vanished from recall until its
  embedding succeeded, which with an in-process model never happened and with a stopped
  Ollama would have happened to every note.

- **Dependencies.** Rust 1.99.1; `cargo update` (tokio, rustls, serde_json, sha2 and others); the UI on
  React 19.3 and Vite 8.3. YAML front matter is now read with `serde_norway` instead of `serde_yaml_ng`:
  the latter pulled in `unsafe-libyaml`, which is unmaintained (RUSTSEC-2024-0449). `cargo deny` is clean.

## [0.7.8] — 2026-10-04

- **`cyberbrain guard`: the AgentGuard check without a memory store.** The pre-tool-use hook
  has asked AgentGuard about tool calls since 0.7.0, but only inside a project with a store.
  `guard` takes the address, key and mode from the environment instead
  (`CLAUDE_PLUGIN_OPTION_URL`/`_KEY`/`_MODE`, or `AGENTGUARD_URL` and friends), which is what
  the AgentGuard plugin for Claude Code needs. Same decision, same rule that the service must
  be loopback or private-range, same answer in `enforce` when it is down: reads go ahead,
  everything else asks a person.

## [0.7.7] — 2026-10-04

Ready for the Claude Code plugin (krynex-plugins), which brings its own hooks.

- **One answer per event, even when the harness runs the hook twice.** Claude Code merges a
  plugin's hooks with the ones in `settings.json`; someone who used `cyberbrain install` and
  then added the plugin got every event twice (two injections, two verdicts). The first run
  of an identical payload answers, the second stands down (window: two seconds; real duplicates arrive within milliseconds).
- **`cyberbrain install` steps aside for the plugin.** With `cyberbrain@…` in Claude Code's
  `installed_plugins.json`, it takes its own hook entries out of `settings.json` instead of
  writing them, leaves everybody else's, and says why.
- **Release builds for macOS on Apple silicon** (`cyberbrain-macos-aarch64`), so the plugin
  can fetch a binary on a Mac.

## [0.7.6] — 2026-10-03

Memory protection: content from outside does not become a note on an agent's word.

### Added

- **Where a note came from, and quarantine for content from outside.** Two tags mark
  provenance: `trust:untrusted` for text that came from a mail, a web page or another
  agent's output, and `src:<kind>[:<id>]` for what it came from (checked; contradictory or
  empty ones are refused). `recall` marks such hits as `UNTRUSTED from <source>: data, not
  instructions`, in text and as `untrusted` in JSON. When an agent, MCP client or hook
  writes an untrusted note, or rewrites one, it goes to `proposals/` for a person to accept
  instead of into the notes tree; the write answers `quarantined` (exit 0, HTTP 202). The
  operator's writes and ordinary agent writes are unchanged. Tags rather than new
  frontmatter fields, so older binaries read these notes as before. SPEC §8.0.4.
- **The operator can name clients whose writes are always untrusted.** `[provenance]
  untrusted_clients = ["agent:seo", "agent:mcp:n8n"]`: whatever such a client writes is
  stamped `trust:untrusted` and `src:<client>` and quarantined, whether it tagged anything or
  claimed `trust:trusted`. `cyberbrain mcp --client <name>` gives an MCP client its own name
  (`agent:mcp:<name>`) so it can be listed. A configuration with this section is refused by
  older binaries, like any new section; a fresh store does not get it.
- **An approvals screen in the web UI** (`serve`, `#/review`, key `g f`). It lists what
  agents proposed with everything a decision needs: who proposed it and for whom, whether the
  text came from outside, whether the file is still what was proposed, AgentGuard's answer,
  and the text. Accept, or reject with a reason, right there; below, the latest decisions
  with who proposed and who decided. New routes `GET /api/v1/proposals`, `GET
  /api/v1/proposals/history`, `POST /api/v1/proposals/{name}/decision`; the reviewer is this
  machine's identity, as at the CLI.
- **Limits per client.** `[clients.bereiche]` maps a client (`agent:mcp:seo`) to the
  bereiche it may see and write: recall returns only notes in them, a citation outside
  them reads as not found, the code index is closed to it, and its writes go only into
  them (an unset bereich is filled in when one is listed). Unlisted clients are unlimited.
  SPEC §9.2.0.
- **MCP over HTTP, with a token per client.** `cyberbrain mcp --http <addr:port>` serves the
  MCP tools at `POST /mcp` for clients that cannot start a process here (n8n in a
  container). `cyberbrain mcp-client add <name>` prints a token once and stores only its
  hash outside the store; each request runs as `agent:mcp:<name>`, so `[provenance]
  untrusted_clients` and the audit log can tell the clients apart. Loopback and private
  addresses only, no browsers (`Origin` refused), stateless. SPEC §9.2.1.
- **Agent proposals can be approved in AgentGuard.** With `[governance] url` set, a proposal
  from an agent, MCP client or hook is also reported to AgentGuard as `memory_write`, which
  always asks an admin (AgentGuard from 03.10.2026). `cyberbrain review <name>
  --from-agentguard` accepts it once that approval says `approved`; the action id is the one
  recorded at propose time, not one the caller names. Needs `memory_write` in the AgentGuard
  agent's scopes.

### Fixed

- **A proposal can no longer be rewritten before it is reviewed.** The file in `proposals/`
  was not guarded: an agent could change a waiting proposal in place, and the reviewer then
  accepted text the proposer never wrote, under the proposer's name. `propose` now records
  the blake3 digest of the file in the hash-chained `note.proposed` row, and `review
  --accept` refuses a file that no longer matches it (rejecting still works). The
  pre-tool-use hook also refuses edits under `proposals/`, for the file tools and for the
  usual shell writes. Proposals made with an older version carry no digest and are
  reviewed as before.
- **An agent's proposal names the agent, and only a person accepts.** Run by an agent, a
  proposal carried the person's identity, so the person was refused by the two-person rule
  and any other name was let through. It now records the agent as proposer and the person
  as `on_behalf_of`. Agents, MCP clients and hooks can no longer accept a proposal in any
  ring (ring 2–4 proposals were theirs to accept before); an agent can still withdraw its
  own.

### Upgrading

Replace the binary. Nothing else is required, and the index schema is still v5. Three things
behave differently:

- **No agent, MCP client or hook accepts a proposal any more**, in any ring; before, ring
  2–4 proposals were theirs to accept. A person does it, at the terminal (`cyberbrain
  review`) or in the new Approvals screen. A pipeline can still complete one through
  `review --from-agentguard` once an admin approved it there.
- **An agent's proposal names the agent** (`agent:<name>`), with the machine's identity as
  `on_behalf_of`. The person can now accept their agent's proposals; before, the two-person
  rule refused them.
- **`trust:` and `src:` tags are checked.** A note with `trust:maybe`, both `trust:` values,
  or an empty `src:` is refused. Other tags are not looked at.

The new `[provenance]` and `[clients]` sections are optional and not written into a fresh
store. A configuration that uses them is refused by 0.7.5 and older, as any new section is.

## [0.7.5] — 2026-10-01

### Added

- **A session starts with the handoff the previous one left.** Handoff notes are usually
  written by an agent at the end of a day, into ring 2 or 3, where `recall` finds them only
  for somebody who already thinks to ask. A fresh session does not: after a reboot one
  checked the machine by hand and never ran the check script the handoff named. Now
  `session-start` shows the newest note in ring 2 or 3 whose name contains `handoff`, after
  the resident rings, with its citations, its date and how old it is, and says that rings 0
  and 1 outrank it. Only on `startup` and `clear`, only if it is younger than a week and not
  invalid, and blocks beyond 1,500 tokens are listed by citation. The new `[handoff]`
  section sets the name patterns (an empty list turns it off), the age and the length. The
  `session.start` audit row names the note it showed.

### Upgrading

Replace the binary. Nothing else is required: a configuration without `[handoff]` takes the
defaults. If your handoff notes are named differently, add their word to
`[handoff] name_contains`, and keep that word out of the names of notes that are not
handoffs, because every note whose name contains it is a candidate. The index schema is
still v5.

## [0.7.4] — 2026-09-29

### Fixed

- **0.7.3 made two hooks slow.** Looking for the resident attestations that 0.7.3 introduced,
  `session-start` and `user-prompt-submit` read the whole audit log on every call: the audit
  reader matches actions as families and so fetched every row to filter it. On a store with
  16,000 audit rows both hooks took about 40 ms against their 15 ms budget (0.7.2: 6–9 ms).
  The lookup now asks the log for the one exact action, which its index answers: 6–8 ms
  again, process start included.
- The hook budget benchmark now runs against an audit log of 20,000 rows. It fails against
  the 0.7.3 lookup and would have caught this before the release.

### Upgrading

Replace the binary. Coming from 0.7.2 or earlier, the two steps under 0.7.3's "Before you
upgrade" apply. The index schema is still v5.

## [0.7.3] — 2026-09-29

Found in a full review of 0.7.2 on 2026-09-29; every item below was reproduced against 0.7.2
before it was changed, and has a test that fails without the change.

### Before you upgrade

- **Run `cyberbrain policy attest` once, read the list, then `cyberbrain policy attest --yes`**,
  in your own terminal. Rings 0 and 1 are now injected into a session only if their files are
  what the operator last wrote there through cyberbrain (below). Notes written by an older
  version have no such record yet, and until you attest them every session reports them as
  "not injected". `attest` without `--yes` records nothing and shows each file with its first
  line: anything on that list becomes an operator instruction, so remove what is not yours.
- **`serve` no longer writes rings 0 and 1** unless started with `--allow-resident-writes`.
  Edit resident notes at a terminal, or start the page with that flag when you mean to.

### Security

- **Rings 0 and 1 inject only what the operator wrote there.** The hook injected any file in
  `notes/r0` and `notes/r1`: one dropped in by a script (`python -c`, `echo >`, a symlink)
  became an operator invariant at the next session start. Every resident write by the operator
  now records the file's blake3 in a `resident.attest` audit row, and a file whose hash is not
  the last one recorded for its path is listed as not injected instead. An agent runs as the
  same user and could still forge a chained audit row; that is a deliberate act where a dropped
  file was not even that. A separate account for agents is what holds against it.
- **`serve` answered every program on the machine as the operator.** `curl -X POST
  /api/v1/notes` with `"ring": 0` came back 201. Resident writes over HTTP are now refused by
  default, and `serve` takes its actor from the environment like the CLI, so an agent that
  starts `serve` is an agent.
- **The daemon believed the actor a client sent.** A script at the socket claiming
  `operator` wrote ring 0. The daemon now reads the connecting process's environment as of its
  start (`SO_PEERCRED`, `/proc/<pid>/environ`, Linux); a process started as an agent is an
  agent.
- **An unclaimed hub could be claimed by any web page** whose name was rebound to 127.0.0.1:
  a loopback peer was taken as "at the machine" whatever `Host` it named. The `Host` has to be
  a loopback name as well, and every browser POST to the hub has to carry the hub's own origin.
- **A revoked hub principal stayed signed in** for as long as the session kept sliding, with
  its old role. A principal's session is checked against the record on every request.
- **An agent could mark its own PII finding as reviewed** (`choice: mark-reviewed` over MCP).
  That choice is the operator's; an agent may redact or proceed flagged.
- **The PII gate missed passwords inside URLs** (`postgres://user:pass@host`,
  `redis://:pw@host`), German labels (`passwort:`, `kennwort:`), and anything after the first
  `!` or `&` of an assigned secret, which a redaction then left standing. Tags skipped the gate
  entirely; a tag with a finding is now refused, since half a tag cannot be redacted.

### Fixed

- **`forget` left the erased text in `cyberbrain.db`.** The rows went, the bytes stayed, in
  freed pages and old full-text segments, so every copy of the store carried what was asked to
  be erased. Freed pages are zeroed now, the full-text index is compacted after an erasure,
  and the write-ahead log is checkpointed and truncated.
- **Parallel writers tore the store apart.** Twenty writes to one name left the file with one
  id and the index with another; a `scan` beside forty writes dropped rows whose files existed.
  Every mutating operation now holds an exclusive lock on `<store>/.lock`.
- **A malformed hub licence took the hub down until a restart**: a panic on a non-ASCII
  character poisoned the hub's lock. It is refused now, and a poisoned lock is recovered.
- **`recall -n` with a huge number aborted the process.** `n` is bounded to 1..1000.
- **A settled note gave no sign of it.** A hit now names up to three notes updated after it
  that link to it (`↳ newer notes link here: …`), so an "open: …" that a later note closed is
  no longer read as still open. `conflict` could not say so when both sat in one ring.
- **`invalidate --clear` left the note marked superseded**, with no command to lift it, and
  `--by` accepted names no note had. `--clear` removes the mark, `--by` needs an existing note,
  and `doctor` reports supersession that points at nothing.
- **`doctor` reported copies pair by pair**: sixty copies of one note were 1,770 warnings. A
  group of copies is one finding now, and at most fifty are listed.
- **A name the validator accepts failed on disk** from 221 bytes, because the temporary file
  added 35 to it.
- **A bad argument exited 2**, which the spec reserves for internal errors. It exits 1.
- **In the web page, `[[wikilinks]]` to existing notes led nowhere** (`href=""`): the
  markdown sanitiser dropped the `wiki:` address before the page could turn it into a link.
- **A store deeper than a socket path allows got no daemon, silently**, and every recall
  loaded the model again (1.5 s instead of milliseconds). The socket moves to
  `$XDG_RUNTIME_DIR/cyberbrain` in that case, and `status` has a `daemon:` line.

### Performance

- **One malloc arena.** A cold `recall` on a 1,500-note store: 1.48 s → 1.20 s, 668 → 570 MB
  (release build, mean of five). A `MALLOC_ARENA_MAX` you set yourself still wins.

### CI

- Runs weekly as well as on push, so a new RustSec advisory is noticed without a commit. The
  workflow token is read-only, every action is pinned to a commit, and Dependabot proposes
  updates to the pins.

### Upgrading

Beyond the two steps above, replace the binary. The index schema is still v5.

## [0.7.2] — 2026-09-28

### Fixed

- **On Windows, the store guard now sees paths written with backslashes.** The
  `pre-tool-use` hook refuses a Bash command that writes into rings 0 and 1 or the audit
  log, but it recognised those paths with forward slashes only. A command such as
  `echo x > C:\…\.cyberbrain\notes\r0\x.md` was let through. Backslashes now count as
  separators on every platform, and on Windows the comparison ignores case, as the file
  system there does. Reads and unrelated paths are still let through.
- **Four embedder tests failed on Windows** because they rewrote the model file while the
  model was still loaded. Since 0.7.0 the weights are memory-mapped, and Windows refuses to
  rewrite a mapped file. The tests now release the model first. Nothing changed in the
  program itself; a model file can still not be replaced on Windows while a process has it
  loaded, which is how that system treats any mapped file.
- CI runs every test binary even after one fails (`--no-fail-fast`). The Windows failure in
  the hook tests had hidden the four above since 0.7.0.

### Upgrading

Nothing to do beyond replacing the binary. The index schema is still v5.

## [0.7.1] — 2026-09-28

### Fixed

- **The governance hook no longer reads the system trust store before every tool call.**
  Every egress request built its HTTP client with the platform certificate verifier, and
  building it read the whole trust store (about 240 files on a Debian or Ubuntu machine)
  even when the destination was plain `http` on loopback, where no TLS handshake can take
  place. On the `pre-tool-use` hook with `[governance]` configured, that was 4 to 5 ms of
  every call and pushed the hook over its own 15 ms budget. A client for an `http`
  destination now gets an empty root store: nothing is read, and a handshake, were one ever
  attempted, would trust nothing. `https` destinations and pinned hub certificates are
  unchanged.

  Measured against a loopback stub, 5 × 200 calls, old and new binary alternating: median
  12.0 → 7.8 ms, slowest 1 % 16–23 → about 10 ms, budget overruns 9 → 0. The hook without
  `[governance]` is unchanged at about 4 ms.

### Upgrading

Nothing to do beyond replacing the binary. The index schema is still v5, as in 0.7.0, so
0.7.0 and 0.7.1 can read each other's stores.

## [0.7.0] — 2026-09-27

### Upgrading: index schema v5, swap everything at once

**This release changes the index schema from v3 (0.6.1) to v5.** The first command of this
version that opens a store migrates its `cyberbrain.db` in place: new columns, no rescan, a
second or less. From then on **an older binary refuses that index** ("newer than this
build"), and there is no downgrade in place. The way back is to delete `cyberbrain.db` and
run `scan --full` with the old binary; the notes themselves are Markdown files and are not
touched.

So the CLI, the hooks, the MCP server and the background daemon must be the same version.
They are usually one file (`/usr/local/bin/cyberbrain`), which makes that easy: copy the
index aside (`cyberbrain.db` with its `-wal` file, or `cyberbrain.db` after the daemon has
stopped), replace the binary, and stop any `cyberbrain daemon` still running from the old
one (a development build may have started one; 0.6.1 had none). A daemon notices a replaced
binary and leaves on its next request by itself, but one that is stopped cannot write into
the migrated index with a writer that does not know the new columns. A machine with a second copy of cyberbrain (an MCP entry or a service pointing
at another path) needs that copy replaced too.

**Run `cyberbrain install` again in every project.** The pre-tool-use hook now also watches
`Bash` and `WebFetch` (see below); a project installed with 0.6.x keeps the old matcher
(`Edit|Write|MultiEdit|NotebookEdit`) until then, and neither the shell guard nor the
governance check sees a shell command there.

### Security

- **An agent at the command line is an agent.** Inside Claude Code (`CLAUDECODE`, or
  `CYBERBRAIN_AGENT` for any other harness) the CLI now opens the store as
  `agent:claude-code:<session>` instead of as the operator. In 0.6.1 an agent that ran
  `cyberbrain write --ring 0` through its shell passed the ring-owner check and the audit log
  called it the operator. It is refused now, and recorded as the agent. `serve` stays the
  operator. Deciding on a ring 0 or 1 proposal is the operator's too: `review --by` is a
  typed name, and an agent could propose a ring 0 note and accept it under a second name.

- **pre-tool-use reads shell commands.** It refuses a shell write or delete aimed at rings
  0 and 1, `audit.db`, the store and its notes tree, and dropping `CLAUDECODE` next to a
  cyberbrain call. It judges each simple command and only the arguments that program writes
  (every path for `rm`/`mv`, the last for `cp`, the files of an in-place `sed`, `dd of=`,
  an extracting `tar`), so reading the store, copying it to `/tmp` or a grep that merely
  mentions a pattern goes through. A heuristic, not a sandbox.

### Added

- **Governance: pre-tool-use asks AgentGuard before a tool call runs.** With `[governance]`
  in `cyberbrain.toml` (url, tenant, agent_id, mode, timeout_ms), Bash, Edit, Write,
  MultiEdit, NotebookEdit and WebFetch calls go to `POST <url>/v1/tool-calls` before they
  run. The key comes from `CYBERBRAIN_AGENTGUARD_KEY` or `~/.config/cyberbrain/agentguard.key`,
  never from the store, and travels as a header. `mode = "shadow"` records the answer and
  stops nothing, not even when the service is unreachable; `mode = "enforce"` carries out deny
  and ask, and with the service unreachable lets plain reads through and asks about
  everything else. The store's own guard decides first. The call is a registered egress
  purpose of its own, `governance`: the configured endpoint only, loopback or private range,
  no redirects, one audit row per call closed with what happened. In shadow the log line
  names what AgentGuard *would* have decided when the service sends it
  (`shadow_permission`, `shadow_reason`): "would be deny (scope_nicht_mandatiert)". A service
  that does not send those fields is logged as before.

- **A note can say from when it holds and from when it no longer does.** `valid_from` and
  `invalid_at` in the head (a date, read as 00:00 UTC, or RFC 3339), `write --valid-from` /
  `--invalid-at`, and `cyberbrain invalidate <name> [--at DATE] [--by NAME] [--clear]`, which
  changes the head only, keeps rings 0 and 1 the operator's, and writes one audit row. Recall
  judges validity at a moment, now or `recall --stand <date>` ("what held on 1 September"):
  a note that does not hold is marked ("[invalid since …]", "[valid from …]"), scored ×0.5
  once, and never hidden. A replacement counts from its successor's `valid_from`, else its
  `created`. The bounds travel with a note pulled from a hub, over MCP (`write` takes
  `valid_from`, `invalid_at` — `null` removes one — and `supersedes`; `recall` takes
  `stand`) and over HTTP (`front.valid_from`, `front.invalid_at`, `front.supersedes`,
  `GET /recall?stand=`).

- **Supersession, and every hit says how old it is.** `write --supersedes <name>` (or
  `supersedes:` / `superseded_by:` in a head) marks a note as replaced: recall still finds
  it, ranked ×0.5 and marked "[superseded by …]". Every hit carries its note's `updated`,
  on the text line as a date.

- **A background daemon keeps the model loaded.** A CLI `recall` spent 1.08 of its 1.4 s
  building the tokenizer. `recall`, `write` and a `scan` with work to do now ask
  `<store>/daemon.sock` first; nobody there, the CLI starts `cyberbrain daemon` and answers
  this one call itself. Measured on a 1,495-note store: recall 1.37 s / 584 MB → 0.01 s /
  11 MB, write 1.57 s → 0.01 s, scan with one changed note 1.51 s → 0.15 s. The daemon
  answers byte for byte what the CLI would have printed, keeps one identity per caller so
  the audit log still names who asked, leaves after 30 idle minutes or when its binary,
  `cyberbrain.toml` or the model changes, and listens on a `0600` socket. A write or scan
  that went out and got no answer is reported as "may or may not have happened", never done
  twice. `CYBERBRAIN_NO_DAEMON=1` turns it off; Windows answers locally. A daemon refuses a
  request of a protocol it does not speak before doing anything, and the client then does
  the work itself, so a daemon left over from another build never half-carries a write.

- **`scripts/retrieval-bench.py`, a reproducible retrieval benchmark.** Lexical against
  hybrid on LongMemEval-S (470 questions, no LLM calls) and on a labelled copy of a real
  store, with recall@k, MRR, nDCG@10 and latency cold, first call and warm through the
  daemon. First run in `eval-local/ERGEBNISSE.md`: LongMemEval-S hybrid r@3 0.934 against
  lexical 0.932; the own store 0.794 against 0.676; warm hybrid through the daemon 8–14 ms
  (p50) against 1.5 s cold.

- **`doctor` names notes that share most of their blocks** ("shared blocks"): 98 pairs in
  one real store, the largest two notes sharing 73 of 77 blocks.

### Changed

- **Recall shows each text once.** Blocks whose text is equal (from 80 characters, case and
  whitespace aside) collapse into one hit, which keeps the best place and score and shows
  the copy from the lowest ring, then the newest note. In one real store 19 % of blocks had a
  twin elsewhere, and twins took 6 of the top 8 places for some questions; now none.

- **An agent's recall leaves rings 0 and 1 out,** and copies of their text with them: its
  session start injects both whole, so a hit from them is a second copy of its context. A
  caveat counts what was left out, `--ring 0|1` still searches them, and a person at the
  terminal, the page or an MCP client gets them as before.

- **Loading the model got cheap where nothing is embedded.** Weights are memory-mapped, a
  verified file is not hashed again while length, mtime and inode are unchanged, and
  `status`, `doctor` and an idle `scan` no longer load the model at all: 2.7 s → 0.1–0.2 s
  each. New dependency: `memmap2` (MIT OR Apache-2.0).

- **The web page got one design system** — tokens, controls, tables, a mobile shell — and the
  audit log and egress register now fit a 1440 px screen without cutting anything off.

### Fixed

- **A recall wrote two audit rows for a contradiction check it then skipped.** The inference
  client was opened (endpoint check, `egress.permitted`) before deciding whether to check at
  all: 27 % of one store's audit log. The check is planned from disk first, and only a check
  that runs opens the client; hits from one ring are no longer booked as a 0 ms check.

## [0.6.1] — 2026-09-23

### Security

- **An agent could write rings 0 and 1 over MCP.** Those two rings are the operator's: every
  session start injects them as invariants that outrank everything else. The hub path refused
  them, but `write` itself never asked who was writing, so `write {ring: 0}` through
  `cyberbrain mcp` put a note into `notes/r0/`, and from then on into every session an agent
  started. A standing prompt injection, one tool call away. Now only the operator (the CLI
  and the web UI) writes rings 0 and 1. MCP and every other actor get a policy refusal that
  names `cyberbrain propose` as the way in, the refusal is a `policy.refusal` row in the audit
  log, and overwriting an existing ring 0 or 1 note under another ring is refused the same
  way. MCP offers no delete, rename or import, so `write` was the only door.

- **`cyberbrain serve` answered under any name, so a web page could read the store.** The
  server binds loopback only and has no sign-in, and a cross-site page could not read its
  answers — unless the page's own name pointed here. With DNS rebinding a site resolves its
  name to `127.0.0.1` after loading, and the browser then treats the API as that site's own:
  `curl -H "Host: attacker.example:17777" http://127.0.0.1:17777/api/v1/notes` returned the
  note list. Every route, the page and its files included, now answers only under
  `127.0.0.1:<port>`, `localhost:<port>` and `[::1]:<port>`, and anything else gets
  `421 Misdirected Request`. Opening the page under another name that points at this machine
  (an `/etc/hosts` alias, a reverse proxy) no longer works, and neither does an SSH tunnel
  whose local port differs from the server's (`ssh -L 8080:127.0.0.1:17777` sends
  `Host: localhost:8080`); forward the same port number, or use one of the three names.

- **A hub answered guessed enrolment codes at full speed and wrote none of them down.** The
  code's randomness made guessing hopeless, but the person running the hub had no way to see
  that somebody was trying. Every refused enrolment is now in the hub's own log as
  `enrolment.refused`, with the reason and the address. An address that tries ten unknown
  codes in ten minutes gets `429` with `Retry-After` until the ten minutes are over, before
  its code is looked at, and each unknown code costs its sender 400 ms. Only unknown codes
  count, so machines holding an invitation that ran out still hear why rather than a lock.
  Because the hub's log cannot be deleted from, one address writes at most twenty refusals
  per window into it. The count is by connecting address (IPv6 by /64): behind a reverse
  proxy one guesser locks everybody out for the rest of the window. `docs/HUB.md` has the
  details. Closes a known gap named in the 0.6.0 release notes.

- **A refused sign-in on the hub's page left no trace.** A wrong password cost 400 ms and
  was otherwise forgotten, so an operator could not tell that somebody had been trying. Every
  refused sign-in, and every wrong current password on the password form, is now a
  `login.refused` line in the hub's own log with the address it came from. A credential that
  was withdrawn and is still being tried is recorded as such, with whose it was, while the
  caller hears the same sentence as for any wrong password. What was typed is not recorded.
  There is still no lockout, on purpose. One address writes at most twenty lines per ten
  minutes, then one `login.unrecorded` line.

- **Note sync and note erasure reached a hub on a public address without
  `allow_public_hub`.** The gate applied the public-address rule to inference, audit
  delivery and enrolment, and not to the two note paths, although the egress register said
  it did. A store whose hub address resolved to a public address had its audit rows refused
  and its notes sent. Every path to a hub now keeps the rule, and a new purpose cannot be
  added without deciding which rule it falls under.

- **The TLS library in 0.6.0 carried a published advisory.** rustls 0.23.43 is covered by
  RUSTSEC-2026-0285, and `cargo deny` had been failing on it since the day after 0.6.0 went
  out; nobody saw it because CI only runs on a push. rustls is now 0.23.45, the only change to
  the lock file. rustls carries every TLS connection cyberbrain makes: to the hub, and to a
  model endpoint when one is configured.

### Fixed

- **Refusing a public hub address named the wrong setting.** The refusal said "the inference
  endpoint must be loopback or private-range unless allow_public_endpoint is set". Following
  it unlocked note text to a public inference endpoint and left the hub refused. It now says
  "the hub" and names `allow_public_hub`.

- **"Already countersigned by" named an id, not a person.** A second countersignature on a
  grant or a purge was refused with "already countersigned by who_01M2…". The same ids stood
  under "Written by" and "countersigned by" on both hub pages, in the log's "Who" column and in
  `hub access-log` and `hub retention list`. All of them show the person's name now, including
  people whose credential has since been withdrawn. The record still stores ids, and `--json`
  output still carries them.

- **A fresh store's compliance overview warned while saying nothing had left.** The page
  knew two egress purposes while the server sends seven. Four of them name their destination
  in words when nothing is configured, "not enrolled" or "wherever you point it", and the
  server classified those words as an unresolved hostname, which the overview warns about. So
  the headline "Nothing has left this machine." came in a warning colour, with the sentences
  listed as "your own network". Such entries are now class `none`, are not listed as a
  network, and do not colour the headline. The page's type and its mock know all seven
  purposes.

- **`hub grant add` with a device the hub does not have said "FOREIGN KEY constraint
  failed".** Most often the device's name had been typed, which is what `hub fleet` shows. The
  refusal now says when a name was given and which id it belongs to, lists the hub's devices
  with their ids when there are twenty or fewer, and refuses a revoked device in words rather
  than writing a grant that could never move a note.

- **The countersigner's log stopped at the hub's first 200 entries.** The page asked for 200
  entries and got the oldest, not the newest, so from entry 201 on nothing new appeared there,
  and `hub access-log` did the same with its `--limit`. For a works council this is the page
  that answers who looked at what. Both show the latest entries now, and the page says when
  there are more than it shows.

## [0.6.0] — 2026-09-12

### Added

- **A note can be written from the page.** The server has taken `POST /api/v1/notes` all
  along; the page never called it, and the empty store told a colleague to open a command
  line. Both views have a "New note" button now. The simple view asks for a title and a text
  and writes into ring 2 as knowledge, shared with nobody. The full view adds ring, kind,
  bereich and tags. The name is made from the title, "Auslieferung für Kunden" becomes
  `auslieferung-für-kunden`, and is shown before saving so it can be changed. A name that
  exists already is refused in words, never overwritten, and a write held for personal data
  gets the same dialog as an edit.

  The page offers rings 2 to 4 only. Rings 0 and 1 are loaded into every agent session on the
  machine and outrank everything else, so setting one stays a deliberate act on the command
  line. That is a choice of the page, not a refusal of the server: the API still accepts any
  ring.

- **`cyberbrain hub backup`, a copy of the record that is checked before anyone relies on
  it.** Copying `hub.db` while the hub runs can miss the rows still in its WAL, and such a
  copy opens, passes every chain check and holds nothing; that is how the first version of
  this command failed its own test. It takes a snapshot through SQLite, then opens the copy
  and checks every chain and that the copy holds at least what the original held a moment
  before. It exits non-zero otherwise, never writes over a file, and puts the backup in the
  hub's own log. `docs/HUB.md` says how to restore one.

- **Fleet invitations: one file enrols a rollout.** One invitation per device is fine for
  three machines and a chore for forty, and every project a person opens is a device of its
  own. `hub invite create --uses 60 --expires P14D --label … --hub-url …` writes one file;
  `hub enrol` with it, or the launcher's **Connect to the company hub…**, asks the hub for a
  device of that project's own, named after the machine and the project folder. The hub keeps
  only the code's hash, refuses a code that expired, ran out or was withdrawn (a withdrawn
  code reads the same as an unknown one), counts seats by machine, and logs every enrolment.
  Nothing is written on the machine until the hub has answered. It is the one request made
  before a store is enrolled, so it is a new registered egress purpose, `hub-enrolment`, that
  reaches only the address in the file. `hub invite list` and `hub invite revoke` manage them.

- **A rollout can leave its fleet invitation on every machine, and the launcher asks once per
  project.** `cyberbrain-setup.exe /S /INVITE=<file>` places the invitation in
  `%ProgramData%\Cyberbrain`, and the uninstaller removes it again. When a project turns out
  not to be connected, the launcher asks whether to connect it to the company hub; yes enrols
  it, no is remembered for that project and never asked again. Nothing is connected without
  that answer, because a project on a company machine can still be somebody's own.
  `docs/ROLLOUT.md` walks through a rollout, and CI now installs over a running installation
  to check that the hub service comes back and the invitation arrives.

- **A seat is a machine, not a project.** Every project is its own store and so its own
  device, and counting devices charged a person with three projects three seats, which is not
  what the licence says. Deliveries now carry the machine's name, the hub counts seats by
  distinct machines, and `hub fleet` shows the machine beside each device. A device that has
  not delivered yet counts as a machine of its own; `hub add … --machine <name>` registers a
  further project on a seated machine without a new seat. The egress register says the name
  and the program version leave with every delivery.

- **A hub keeps activity rows for a set period, and removing older ones takes two people.**
  Until now it kept every row for ever: the record was append-only with no way out, which a
  works agreement cannot accept. `hub retention set P2Y` sets the period and removes nothing.
  `hub retention propose --reason …` writes a purge down, and it is carried out when a
  countersigner signs it, on the command line or with a button on `/requests`. What goes is
  the start of each device's chain up to the first row that is not old enough, never a
  selection, so a device whose clock once ran backwards keeps an unbroken chain; the hash of
  the last removed row becomes the floor `hub verify` checks from. Every step is in the hub's
  own log. Backups keep purged rows until they are rotated, which `docs/HUB.md` says plainly.

- **A resolved conflict no longer keeps the text of the version that lost.** The decision
  stays on record; the second copy of a department's text does not.

### Changed

- **A note name may carry accented Latin letters.** `auslieferung-für-kunden`, `straße` and
  `łódź` are names now; until this version they were refused, and a German team wrote "fuer"
  into every name. Names stay lowercase, with digits and single hyphens, at most 120
  characters and 240 bytes, because a name is also a file name.

  Two things come with it. `ü` has two spellings in Unicode that print alike and are
  different file names, so every name taken from outside, in a write, a lookup or a
  `[[link]]`, is composed to NFC first, and the other spelling finds the same note instead of
  making a second one. And letters of other scripts stay out: a Cyrillic `а` cannot be told
  from a Latin `a`, and a name that reads like another note's is a way to put words in its
  place.

  `cyberbrain import` still spells umlauts out, on purpose. A repeated import finds its notes
  by name, and a different name for the same heading would write a second note on every run.

  **Update every machine on a hub before the first such name is shared.** An older machine
  cannot write it; see the pull fix below for what it does instead from this version on.

### Fixed

- **Two projects on one machine enrolled with the same hub delivered as one device.** The
  token was kept per user and hub address, so the second `hub enrol` overwrote the first
  project's token; both stores then delivered as the second device, and the hub turned one
  of the two chains away at its anchor, which looked like a machine that had gone quiet. The
  pull position and the list of known notes were shared the same way, so a second store could
  skip notes. All three are kept per device now. A store enrolled with an earlier version
  keeps working from its old file. **If you enrolled two projects with one hub before this,
  enrol the first one again** with its own invitation: its old file holds the other's token.

- **One note a machine could not write stopped the whole hub pull, for good.** The notes after
  it never arrived, the cursor did not move, and every later pull stopped at the same place.
  Such a note is refused and named now, the rest of the delivery arrives, and the cursor waits
  so the note is offered again once the machine can take it. A machine still on an older
  version keeps the old behaviour, which is why the note above asks to update first.

- **The Windows launcher told a person who had just enrolled that what a note says never
  leaves the computer.** That stopped being true with note sharing in 0.5.0, and the docs were
  corrected then; the launcher's dialog was not. It now says what enrolment sends, and that
  sharing notes is a separate setting that is off.

- **The question page said "Everything is in order" when it could not find out.** The health
  line only checked the status when it had one, so a failed status call left a green dot. It
  now says the state could not be checked.

## [0.5.1] — 2026-09-11

### Fixed

- **A note could be written and still be missing from recall.** With several `cyberbrain
  write` running at once on macOS or Windows, one could stop with `index: database is
  locked` after its file and its audit row were already there. The index began its writing
  transactions deferred, so SQLite refused the lock straight away instead of waiting the five
  seconds that had been set for exactly this. It takes the lock at the start now, where the
  wait applies. Linux never showed it, at eight writers or at thirty-two. A note left behind
  this way comes back with `cyberbrain scan`, which indexes every file it has not seen.

- **A debug build on Windows overflowed its stack before it had parsed anything.**
  `cyberbrain --version` was enough. The command tree had grown past the 1 MB Windows gives
  a main thread; Linux gives 8 MB. The released binaries fit and were not affected, but every
  Windows test that starts the binary was. The program now runs on a thread whose stack it
  chooses itself.

## [0.5.0] — 2026-09-10

### Added

- **Notes can travel between machines, through the hub.** Until now the hub took audit rows
  and nothing else. A store that switches on `allow_note_sync` can share the notes that
  carry a `bereich` — a department, team or domain — with the devices that have been granted
  that bereich. Rings 0 and 1 never travel, whatever is granted, and that is checked on the
  way out, on the way in, and by a constraint in the hub's own database.

- **A bereich grant takes two people.** One person writes it and a second countersigns it
  with `cyberbrain hub grant approve <id> --as <credential>`; until then it is written down
  and moves nothing. Whoever runs the hub registers the devices and can read a device token
  out of the invitation file, so a single-signature grant would have meant that the operator
  could point any department at a machine of their own — and "admin sees state, not content"
  would have been a house rule rather than a property of the machine. **Upgrading a hub makes
  its existing grants inactive**, because treating what is already there as signed would
  carry that hole over the upgrade. Each refusal names the one command that revives one.

- **A fourth role, `editor`.** Two machines that changed the same note without seeing each
  other produce a conflict, and deciding which version stands means reading both texts. That
  is a job for somebody who knows the work, not for whoever runs the machine — so `admin`
  still sees no note text, and an editor sees conflicts only in the bereiche they were
  assigned.

- **`bereich` on a note**, set with `--bereich`, in the frontmatter, in the web UI, over MCP
  and over the API. It filters recall and never ranks it.

- **The hub has a page for the roles it depends on.** Signing in with an auditor or
  countersigner credential opens `/requests`: an auditor asks there and sees their own
  requests; a countersigner sees every request with its reason, the bereich grants waiting
  for a second signature, and the hub's own log, and acts on them with a button. Until now
  those two roles had a credential, a login box that accepted it, and nowhere to go — their
  work was reachable only from a shell on the hub's own machine, which is the machine whose
  operator they are there to check.

- **Age is a tie-break in recall.** Between two hits a ring apart, the ring still decides;
  between two hits of the same ring, the fresher one comes first. Deliberately smaller than
  the closest gap between two ring weights, so it can never reorder across rings.

### Fixed

- **A note could exist that the audit chain did not mention.** Two writers arriving at once
  — a hook, an MCP server and a terminal are three processes, not three threads — raced for
  the end of the chain, and the loser was told "nothing was written" after its file was
  already on disk. Eight parallel writes left eight notes and one row. The chain append now
  rebuilds the row against the head that is actually there and offers it again, and if it
  still cannot get in, the note file goes back to what it was.

- **Any signed-in principal counted as the hub administrator.** The hub has one login box
  and two kinds of caller behind it; the question asked of the cookie was only "is this
  session live". An editor could hand themselves a grant to any bereich on the hub. The role
  decides now.

- **`cyberbrain hub conflicts` printed every department's note text** to whoever could open
  the file, while the web page checked the role and the bereich for the same rows. It takes
  an editor's credential now.

- **`cyberbrain hub report` wrote out every device's activity rows** with no credential and
  no record, one subcommand away from the disclosure route that needs an auditor to ask and
  somebody else to approve. It writes the summary only; the rows are `hub disclose`.

- **Devices arrived and left without a word in the hub's log**, although granting a role had
  been an entry all along.

- **A pulled note lost its identity.** It arrived with a new id (so a citation written on the
  sending machine resolved nowhere), `created` of now (so a retention clock restarted), no
  tags, no retention at all, and `updated` of now — which made the next real change from the
  other machine look older than a note this machine had never touched.

- **An ordinary edit dropped a note's retention period.** `retention` could not tell "not
  mentioned" from "remove it", so fixing a typo silently removed an agreed deletion date.

- **Erasing a shared note said nothing about the hub's copy** except at the command line.

- **Signing in as anything but the operator was a dead end.** The login sent everybody to
  `/`, which is the operator's page, so an editor, an auditor and a countersigner signed in
  successfully and were handed the login form again — indistinguishable from a wrong
  password. Each role lands on its own page now. `/login` answered 405 to a GET, which is
  what the two redirects for an unauthenticated visitor pointed at; it is a page.

- **Quitting the Windows launcher left `cyberbrain.exe` running**, which blocked the next
  installation. The launcher now holds every process it starts in its job object, answers
  `cyberbrain-desktop.exe --quit`, and the installer asks it to close before replacing
  anything.

### Changed

- **`docs/HUB.md` and both READMEs said that what a note says never leaves the machine that
  holds it.** That was true when it was written and stopped being true with note sync. It is
  the product's strongest promise, so it is worth saying plainly that it was carrying a claim
  the code no longer supported.

## [0.4.0] — 2026-09-09

### Added

- **The hub encrypts itself.** It used to bind `0.0.0.0:7788` and serve plain HTTP: every
  device token and the administrator password went over the wire as text, and the answer in
  the documentation was to put a reverse proxy in front — which, for a Windows service on a
  machine in a company of eleven people, is a second piece of software nobody installs.

  `hub serve --tls-cert --tls-key` now serves https itself, and on Windows the same two flags
  become part of the service registration, so an upgrade cannot quietly drop back to plain
  text. Nothing new entered the dependency graph for it.

- **It makes its own certificate**, because the customer this is for has no certificate
  authority and telling them to obtain one is telling them to stay in plain text.
  `hub serve --tls-generate`, and the Windows installer does it for you unless you supplied a
  certificate or asked for `--insecure-http`. The pair lives beside the record, the key
  readable only by the machine's administrators.

- **An invitation says which certificate to expect.** Every invitation issued while such a
  hub is running carries its fingerprint, and a client enrolled with one accepts that
  certificate and nothing else — not the platform trust store, not a company CA — and refuses
  to deliver over plain http, where no certificate is presented at all. Nothing has to be
  installed on the client machines: the invitation they were already handed is the channel.

  Only a certificate the hub made itself is pinned. One you supplied has an issuer, and
  issuers renew; pinning it would turn the next renewal into every client stopping at once.

- **`hub cert show` and `hub cert export`**, because a browser is the one client that cannot
  be told anything at enrolment. `show` prints where the certificate is, its fingerprint, and
  the command that makes the machine trust it — for the platform it is running on, and only
  that one.

- **`/HUB` for a silent install.** `/S` could only ever install the default set, which
  deliberately excludes the collector, so an unattended rollout could not produce a hub.

- **The install directory goes on the machine PATH**, and comes off again on uninstall.

### Changed

- **Without a certificate the hub still collects**, and says so: it warns at every start on a
  network address and the page carries a banner. One thing is refused outright — the
  administrator password may then only be typed at the machine the hub runs on, checked
  before the password is looked at rather than after. Deliveries are unaffected, because a
  hub that stopped collecting until somebody produced a certificate would be the worse trade.

- **Registering the service on a network address** now makes a certificate rather than asking
  nothing and listening in the clear. `--insecure-http` is how you ask for plain text.

- **`attempt to write a readonly database`** — what `hub add` says in an ordinary prompt,
  because the record belongs to the service account — now names the way out.

### Security

- **A hub's private key was readable by every account on the machine.** It inherited the ACL
  of `C:\ProgramData`, which grants `BUILTIN\Users` read; anybody who could log in could read
  the key and then be the hub to every machine that had pinned it. The key is now created
  with a DACL that inherits nothing and grants only SYSTEM and the local administrators —
  created that way rather than tightened afterwards, since between a create and a chmod the
  file is readable by everybody. Found by an operator with `icacls` on a real install.

### Fixed

- **A hub did not recognise its own certificate** when the service was registered with paths
  to it, which is exactly what the installer does. It therefore issued invitations with no
  pin, and clients would have refused the certificate nobody told them to expect. Found on
  the first Windows machine this reached, in a log line that stopped one clause short.

## [0.3.1] — 2026-09-08

### Added

- **A simple view, and it is what the page now opens with.** Eight sidebar entries, `g s`
  hints beside each of them, a search box whose placeholder read "Recall… (hybrid: lexical +
  semantic, fused, ring-weighted)", and a ranked list of hits carrying citations and scores:
  every one of those is right for whoever runs the store, and every one of them is wrong for
  the colleague the store is installed next to.

  Simple mode has three entries. A question is answered *once*, at reading size, with its
  sources demoted underneath — because which hit wins is the question the rings already
  answer, and it is the wrong decision to hand to somebody who has never seen a ring. Tiers
  are named in words ("Always applies") rather than as `r0`. No citation, score or command
  name appears anywhere in it, and a browser test asserts that rather than trusting review.

  The full view is one switch away in the sidebar and is remembered. Nothing was removed from
  it: every other screen stays reachable by address, unchanged, so the terminal token in the
  launcher's URL and any link to Status keep working. See SPEC §13.1.

- **The empty store says it is empty.** A machine on its first day showed "no note matches",
  which is what a filter says when it has hidden something. It now says nothing is written
  down yet, what belongs there, and the one command that puts something there.

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


- **`cyberbrain manifest`.** Semantic search needs a model, a model needs a `manifest.json`
  naming the blake3 digest of each of its files, and there was no way to produce one: the
  field names appeared in no Markdown in this repository, and the hashing existed only inside
  tests. The shape had to be guessed from a deserialisation error. It is one command now, and
  it says what to do next — vectors are written when a note is indexed, so `scan` has to run
  again, which was also not written down anywhere a person would look.

### Fixed

- **A decided proposal kept vouching for its own name.** `review` asked the audit log whether
  a `note.proposed` row existed for a name, and those rows never leave the log because the
  chain is hashed — so a proposal that had been rejected months ago still answered. Anyone
  could put a file of that name back into `proposals/` with any content and any ring, and
  `--accept` would find the old row, apply the two-person rule against somebody who had
  nothing to do with it, and write an unapproved ring 0 note whose audit trail then named
  that person as its proposer. The state of a name is now the newest of proposed, accepted
  and rejected, and only `proposed` is an open proposal.
- **The README described a download that does not exist.** It said nothing is fetched on your
  behalf "unless you set `embedding.model_source` and `embedding.model_download_consent` —
  and even then it happens once, through the one registered outbound path". There is no such
  path in the program: `download()` has exactly one caller in the whole workspace and it is
  that function's own unit test. `policy consent --grant` even accepts the consent and warns
  that nothing will happen without a `model_source`, which reads as though setting one would
  help. On a product sold on a register of what can leave your machine, a sentence describing
  an outbound path that is not there is the worst kind of wrong sentence. The README now says
  what is true: nothing is downloaded, ever, and here are the three steps you take yourself.
- **A web page you had open could delete your notes.** `POST /policy/retention/apply
  ?dry_run=false` erases every note past its retention, and a self-submitting form on any
  website reached it: a form post is a "simple request", so nothing preflighted it and
  nothing checked where it came from. `POST /scan?full=true` the same. The other writing
  routes were protected only because `axum::Json` insists on `application/json` and thereby
  forces a preflight — an accident of an extractor, not a decision, and the accident had two
  holes in it.

  State-changing requests are now checked in one place: an `Origin` that is not this page's
  is refused, and so is a `Sec-Fetch-Site` that is not `same-origin` or `none`. Neither header
  means a program on this machine, which is allowed — it can read the store off the disk
  anyway. Reads are untouched, because a cross-site read cannot see its own answer.
- **The terminal token was readable by every account on the machine, twice over.** The
  origin check keeps other *pages* out; the token is the only thing keeping other *accounts*
  out, because loopback is not per-account. So anywhere the token can be read is a hole, and
  there were two. `serve --terminal` handed the address to `xdg-open`, from where it became
  part of the browser's own command line and `/proc` published it for as long as the browser
  ran; it now prints the address and says why it is not opening it. And the
  saved-connections file was written world-readable, while the hub tokens in the same
  directory have always been owner-only — which made the token on that route pointless, since
  the same list could be read straight off the disk.

  The reasoning in the code said "no origin means a program running as *this* user". It
  means *any* user of this machine. The decision it supported is still right; the sentence
  was the place both holes lived, and it now says what it means.
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
- **Clicking any other screen killed every open terminal.** The panes lived in that screen's
  own state, so leaving it unmounted them, closed the sockets and made the server kill the
  processes behind them. Somebody's ssh session ended because they looked at Status. Every
  other screen is a view of the store and costs nothing to rebuild; this one owns running
  processes, so it stays mounted once it has been opened.
- **The terminal was silent to a screen reader.** xterm builds no accessibility manager
  unless it is asked to, so what a screen reader got was a textarea with a fixed English
  label and nothing else: no output, no prompt, no exit. It is asked to now, and each pane
  carries a name saying what is running in it.
- **Neither new input showed a keyboard focus ring**, because a Tailwind `outline-none` had
  quietly overridden the rule that draws one for everything else.
- **The command line took the focus away while a command ran.** Disabling the focused element
  makes the browser drop focus to the body, and keystrokes then reached the global shortcuts
  — a typed `g t` navigated away mid-command. It is read-only while busy instead. Its output
  is also a live region now, and its refusals are announced: with a command line the output
  is the whole of the answer.
- **Refusals were shown as raw JSON.** The terminal screen printed the response body into a
  paragraph of prose; the sentence inside it is the one somebody wrote to be read.
- The keyboard help called the two new screens `console` and `terminals` in both languages,
  because the translation for their names was the one thing not added with them.
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
- **A terminal left its children running, and a thread waiting on them for ever.** Killing a
  session killed only the shell. Anything it had started kept running — and those survivors
  hold the terminal open, so the reader thread behind the closed pane waited for an end of
  file that would never come, holding a thread and two descriptors for the life of the
  process. One closed browser tab, one leak. The whole process group goes now; a grandchild
  that calls `setsid` for itself still escapes, which is what `nohup` and `tmux` are for.
- **Every program in a terminal inherited the terminal itself.** The master side of the pty
  had no close-on-exec, so a shell — and everything it started, to any depth — held a
  writable handle on its own terminal: enough to forge output the page renders as the
  program's, and to read input meant for something else.
- **Choosing a subdirectory of an open project started a second server on the same store.**
  A project is picked by folder, but a store is found by walking upwards — so `work/api` and
  `work/api/src` are two folders and one store, and the "already open" check compared folders.
  Two servers on one store is two writers on one index, which is exactly what that check
  exists to prevent. It compares stores now, resolved through the filesystem so that two
  spellings of one folder are one place.
- **`hub service uninstall` did not wait for the service to stop**, so the delete that
  followed usually landed while it was still stopping and left it marked for deletion until
  the next reboot — the state the comment above that line says it is avoiding. And
  `hub service install` treated every failure to open an existing service as "there is no
  such service", so somebody without the rights to configure one was told the service could
  not be registered, about a service that was there all along.
- **Six things in the Windows-only code, all of the same family: resources with no owner.**
  None of it can be run here, all of it was found by reading.
  - A server that stopped on its own left its window standing — showing a dead address, with
    the launcher no longer holding the handle, closable only by hand. The entry was removed
    without being shut.
  - `exited()` compared the exit code against `STILL_ACTIVE`, which is 259, so a program that
    legitimately exits with 259 counted as running for ever and its pane just went quiet. The
    liveness question goes to `WaitForSingleObject` now; the code is read only after it.
  - WebView2 controllers were released and never `Close()`d, so the browser process tree
    outlived every window — in a program that sits in the notification area for days.
    Including the half-built case: three panes where the third fails no longer strands the
    first two.
  - Three ConPTY failure paths leaked two kernel handles each, and one leaked the attribute
    list as well. A saved connection pointing at a program that is not installed is one
    click, and people click it more than once.
  - `ProjectWindow` decided whether its window was still there by asking `IsWindow` about a
    raw handle. Windows reuses handles, so a stale one can start answering for somebody
    else's window — and then Open focuses the wrong window and closing the project destroys
    it. A window found closed is now forgotten.
  - `SetTimer` with no window ignores the id it is given and returns its own; passing the
    original back to `KillTimer` killed nothing. Two smaller ones with it: two Close events
    for the same project in one tick took the same index twice, and switching Open in › The
    web browser closed the side-by-side window, which has nothing to do with it.
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

[Unreleased]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.10...HEAD
[0.7.10]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.9...v0.7.10
[0.7.9]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.8...v0.7.9
[0.7.8]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.1...v0.7.2
[0.7.1]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.6.1...v0.7.0
[0.6.1]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.5.1...v0.6.0
[0.5.1]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.3.1...v0.4.0
[0.3.1]: https://github.com/bassprofressor-lab/cyberbrain/compare/v0.3.0...v0.3.1
