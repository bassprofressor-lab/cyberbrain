# The hub: one machine collects what the others did

A team keeps its notes where it always did — on each person's machine, in each person's
store. What the hub collects is the *audit trail*: which machine did what kind of thing,
when, in a chain that cannot be edited afterwards without it showing.

It is a second surface, not `serve` with the address opened up. `serve` binds loopback and
has no authentication, and SPEC §8.2 ties those together deliberately: there is nothing to
authenticate because there is no remote access. It can also read, write, delete and apply
retention — opening its bind would hand that to everyone on the network. The hub
authenticates, and the only thing it can do is take rows.

## The licence

A hub needs one, and without it nothing can be registered and nothing is collected. It is a
signed file, checked against a key compiled into the binary — **no call home**, because the
networks this runs on are often deliberately closed and because a licence server is a way for
somebody else's outage to stop your evidence.

```console
$ cyberbrain hub licence install licence.txt --data /var/lib/cyberbrain/hub.db
installed: Beispiel GmbH — 3 seat(s), until 2027-01-01T00:00:00Z

$ cyberbrain hub licence show --data /var/lib/cyberbrain/hub.db
licence: Beispiel GmbH, 3 seat(s), until 2027-01-01T00:00:00Z
seats: 2 of 3 in use
```

`licence show` exits non-zero when the hub is not collecting, so a monitoring check is one
line.

### Seats are devices

Counted at enrolment, not at delivery: a device that was allowed to enrol and is then refused
every night looks registered and collects nothing, which is the worst of both. Revoking frees
a seat — the rows stay, the person left.

### When it expires

From 30 days out, everything that shows state says so, with the days remaining. After the end
date the hub **stops accepting rows and does nothing else**:

- the record stays readable and exportable, nothing is deleted or locked
- devices keep working locally, exactly as before
- clients buffer, and deliveries come back `503` with an explanation — not `402`, because the
  sender did nothing wrong and should retry later, which is what that code tells every
  retrying client there is
- renewing takes what they held, and the chain closes without a gap

That last point is tested, not just intended: a delivery refused during a lapse arrives in
full once a new licence is installed.

## Running one

```console
$ cyberbrain hub add "laptop-anna" --data /var/lib/cyberbrain/hub.db
device dev_01M1Y70BYZM68NWYVE03ZF09YM registered as "laptop-anna"
token: cbh_01M1Y70BYZR5FP0SCQJCGR47WT

This is the only time the token is shown. The record keeps a hash of it.

$ cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db
cyberbrain hub: http://0.0.0.0:7788/  (record: /var/lib/cyberbrain/hub.db; devices
authenticate with a bearer token)
```

### Invitations

Rather than printing a token loose, `hub add` can write an invitation carrying everything the
machine needs — including the shared inference endpoint from
[`docs/SHARED-INFERENCE.md`](SHARED-INFERENCE.md), so that address is not typed into every
store by hand:

```console
$ cyberbrain hub add "ws-021" --data hub.db --invite ws-021.json \
    --hub-url https://hub.example.internal:7788 \
    --inference-url http://192.168.1.50:11434/v1
```

The file carries the token: hand it over the way you would a password, and delete it once the
machine is set up. Reading it on the client is the next slice.

## As a service

A hub is only as useful as it is boring, and a program that is up only while somebody is
logged in makes silence useless as a signal: you could never tell whether the hub was asleep
or the client had stopped. So it runs as a service — and on Windows, setting that up is a
tick box rather than a command.

### Windows

Tick **Hub service (collector)** in the installer. It is off by default, because most
machines are clients and on those it would open a port for nothing. Ticking it:

- creates `C:\ProgramData\Cyberbrain\` and puts a note in it explaining what goes there
- registers the service **Cyberbrain Hub**, set to start automatically, listening on
  `0.0.0.0:7788`
- opens that port in Windows Firewall — without this the hub listens and nothing ever
  arrives, which looks exactly like every client being broken
- adds a Start menu shortcut to the data folder

Then **licensing it is copying a file**: save the licence you were sent as
`C:\ProgramData\Cyberbrain\licence.txt` and restart the service. It is picked up on start
and what it found is written to `hub-service.log` beside it — including the case where it
found nothing, which says where it looked. `licence.txt.txt` and `license.txt` are taken too:
Explorer hides known extensions, so somebody saving an attachment as `licence.txt` gets the
first of those and is shown the name they intended, with no way to see what went wrong. Nobody has to open a command
prompt to put a hub into service, which is the whole point — the moment a setup needs one,
the person who needed the product most is the person who stops.

For administrators who would rather see the command, and for unattended rollouts, the same
registration is one line:

```console
> cyberbrain hub service install --data C:\ProgramData\Cyberbrain\hub.db --addr 0.0.0.0:7788
> cyberbrain hub service status
Cyberbrain Hub is running
```

`status` exits non-zero when it is not running, so a monitoring check is one line. `start`,
`stop` and `uninstall` do what they say; `uninstall` removes the registration and **leaves
the record alone**, as does removing the program. Deleting the software must never delete
the evidence it was collecting.

`hub serve` is the same executable in both roles. Started by the service control manager it
behaves as a service; started from a prompt it is an ordinary console server. There is no
flag, because a flag is a thing to get wrong, and there is no second executable, because two
of them drift.

### Linux

```ini
# /etc/systemd/system/cyberbrain-hub.service
[Unit]
Description=Cyberbrain Hub
After=network.target

[Service]
ExecStart=/usr/local/bin/cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db
Restart=on-failure
User=cyberbrain
StateDirectory=cyberbrain

[Install]
WantedBy=multi-user.target
```

A licence dropped at `/var/lib/cyberbrain/licence.txt` is picked up on start here too.

## Encrypting it

Give it a certificate and a key, in PEM, and it serves https itself:

```console
$ cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db \
    --tls-cert /etc/cyberbrain/hub.pem --tls-key /etc/cyberbrain/hub.key
cyberbrain hub: https://0.0.0.0:7788/  (record: …)
certificate SHA-256: 63:4E:3F:E0:…
```

On Windows the same two flags go on `hub service install`, where they become part of the
registration — so an upgrade cannot quietly put the hub back into plain text.

The fingerprint is printed at every start because it is what somebody compares against what
their browser shows. Clients need nothing: a delivery verifies against the machine's own
trust store, so a certificate from your CA or a public one is trusted the moment that
machine trusts it, by the same rules as everything else on it.

**Without a certificate the hub still collects, and says so.** On a network address it logs a
warning at every start and the page carries a banner, because on that hub every device token
crosses the network in the clear. One thing is refused outright: **the administrator password
can then only be typed at the machine the hub runs on.** Signing in from a desk needs https.
The refusal is deliberately narrow — deliveries are unaffected, and a hub that stopped
collecting until somebody produced a certificate would be a worse trade than the one it is
trying to prevent.

Registering a *new* service on a network address without a certificate is refused, because
that is the one moment a person is standing there to decide. `--insecure-http` says you meant
it. A hub that is already running is never stopped over this.

## The hub's own page

`http://localhost:7788/` on the machine the hub runs on. It answers the three questions
somebody has on the day they install one: is this machine a hub, was the licence accepted,
and is anything reporting.

- **Licence** — collecting or not, the customer, the end date, and the warning inside the
  last thirty days. A licence file lying next to the record is offered as one button; there
  is a box to paste one into as well. Nothing is sent anywhere: the signature is checked
  against a key built into the program.
- **Devices** — the same list `hub fleet` prints, trouble first, and a form that registers a
  machine and writes its invitation next to the record.

### Signing in

The account is `admin`. **There is no default password**, because a default on something that
listens to the whole network is the thing that gets found, and "change it afterwards" is a
sentence people read after the change was needed.

Instead the first visit **from the machine the hub runs on** asks you to set one. That is
safe without a password in front of it: whoever is at that console could read the record with
any SQLite tool. After that the page is reachable from any desk on the network — **provided
the hub is encrypted**, because otherwise the password would cross that network in the clear;
on a hub without a certificate, signing in stays at the machine (see *Encrypting it*). Until
a password is set, the hub still collects — evidence must not wait for an administrator.

Forgotten it, or the person who set it has left:

```console
$ cyberbrain hub admin reset --data /var/lib/cyberbrain/hub.db
```

The next visit from the hub's own machine sets a new one. It needs access to the record,
which is access to the machine, which is the same thing that would let anyone read the record
directly — so this hands out nothing that was not already there.

The password is stored as an argon2 hash with its own salt, sessions live in memory (a restart
signs everybody out, which is what you want after an upgrade), and a wrong password costs a
fixed delay rather than a lockout: locking out an administrator is a way to take a hub away
from the person who runs it.

**This is not the roles model.** It is one account for the machine's administration, which is
what the fleet view is. Nothing reachable with it can read an activity row — that still needs
an auditor, a reason and a countersignature, further down this page.

`/health` and `/api/v1/ingest` are unchanged: delivery is what the network side is for, and
devices authenticate with their own tokens.

It is server-rendered and has no JavaScript. The store's web UI is a built bundle behind a
feature flag and building it needs node; none of that may be the price of finding out whether
a licence was accepted.

## Delivering to it

On the client, once:

```console
$ cyberbrain hub enrol ws-021.json
enrolled with https://hub.example.internal:7788 as dev_01M1Y9…
token stored at ~/.config/cyberbrain/hub-tokens/4a920a2c….token
inference endpoint set to http://192.168.1.50:11434/v1
```

On Windows, neither of those is a command anybody has to find. The tray menu has **Connect
to the company hub…**, which takes the invitation file, and after that the launcher delivers
by itself: half a minute after it starts, then every quarter of an hour, and once more on the
way out. It asks a store exactly once whether it belongs to a hub — most do not — and it says
nothing when a delivery fails, because a train or a hotel wifi is not news and a dialog on
every one of them teaches people to dismiss dialogs. A hub that stops hearing from a machine
sees that in its own fleet view, which is where it belongs.

Everywhere else, and for machines that are not running the launcher, on a timer — once an
hour is plenty:

```console
$ cyberbrain hub push
delivered 11 new row(s) to https://hub.example.internal:7788; the hub now holds 11
```

**The token is not in the store.** `cyberbrain.toml` lives inside the store, a store is meant
to live in a repository, and a credential there gets committed by the second person who runs
`git add .`. It goes into the user's configuration directory instead, one file per hub, or
into `CYBERBRAIN_HUB_TOKEN` for a service account.

**Nothing is buffered separately.** The audit log *is* the buffer: it already holds every row
in order, and the hub says where it stopped. A failed delivery changes nothing locally — the
next one covers the same ground plus whatever happened since.

`hub push` exits non-zero only for something an operator has to fix. A hub that is not
collecting (an expired licence) and a gap that a wider period would close are states a timer
should see and carry on from, and they exit 0 with an explanation.

Every delivery is itself audited, so a push writes two or three rows of its own — which the
next one carries. On a quiet machine that is the heartbeat: rows keep arriving, so silence
means the client stopped rather than the person did.

### The path is in the register

`audit-sync` is a registered egress purpose, which means `cyberbrain policy egress` lists it
whether or not this store is enrolled, says whether it is enabled, and states in as many
words that it does not carry note content. The gate refuses any destination that is not the
hub this store enrolled with, so editing the URL in the config file does not redirect a
company's audit trail — it produces a refusal, and the refusal is itself a row.

### By hand, if you prefer

```console
$ cyberbrain policy audit --export period.jsonl
$ curl -X POST https://hub.example.internal:7788/api/v1/ingest \
    -H "Authorization: Bearer $TOKEN" --data-binary @period.jsonl
{"accepted":4,"device":"dev_01M1…","next_anchor":"d1e0359c…","total_rows":8}
```

### Overlap is fine, a gap is not

Each device's rows form one chain, and the hub remembers the hash of the last row it took
from that device. A delivery is accepted when it *contains* that row: everything after it is
new, everything before it is overlap and is skipped.

That distinction is the whole design. A client cannot know the hub's anchor without asking
first, and asking would make every delivery two round trips and still race — so a client that
resends the last week after a bad connection is behaving correctly, and sending the same file
twice changes nothing the second time (`"accepted": 0`). What cannot pass is a delivery that
does not contain the anchor at all: that means rows are missing between the two, and it is
refused with `409` naming both hashes.

| Situation | Answer |
|---|---|
| Continues exactly | `200`, rows appended |
| Overlaps what the hub has | `200`, only the new part appended |
| Same file again | `200`, `"accepted": 0` |
| Rows missing in between | `409`, with `expected_anchor` and `got_anchor` |
| Edited or broken bundle | `400`, naming the row and what is wrong with it |
| No token, unknown token, revoked device | `401` |

An empty delivery is accepted and moves `last_seen`. A device with nothing to say should look
different from one that has stopped saying anything.

## What the hub knows, and what it does not

It holds the same rows the client's own audit log holds: timestamp, actor, action, subject,
and the detail as written. **It has no notes**, no index and no search — what a note *said*
never leaves the machine that holds it.

`cyberbrain hub fleet` shows devices, not activity — and shows the ones with something wrong
first, because a list that reads the same whether or not there is a problem gets skimmed:

```console
$ cyberbrain hub fleet --data /var/lib/cyberbrain/hub.db
!!  build-01                      0 rows  never reported
!!  ws-014                    4 511 rows  last delivery refused at 2026-09-05T02:11:04Z: this device's chain is at 4005a9b0…
!!  ws-007                   15 902 rows  quiet for 71 h; version 0.1.0, hub runs 0.2.1
ok  laptop-anna              12 480 rows  last seen 2026-09-07T15:17:29Z
-   laptop-old                   31 rows  revoked

32 924 row(s) in the record, 3 device(s) need attention
licence: Beispiel GmbH, 5 seat(s), until 2027-01-01T00:00:00Z
```

**A refused delivery is remembered.** Without that, a gap would be invisible here: a delivery
that does not continue the chain is turned away, so it leaves no rows, and the device would
look merely quiet — a different problem with a different fix. A later good delivery clears
the note, because a stale complaint is worse than none.

A revoked device raises nothing: that is a decision somebody made, not a fault. Its rows stay,
because revoking is not a deletion.

## Who may see what

Two questions that tend to land on one screen, kept apart: *is the collection working* is
daily administration, *what did this person do* is a procedure. The second is modelled on how
works agreements handle access to video recordings — not forbidden, but never alone and never
unnoticed.

| Role | Sees | Does not see |
|---|---|---|
| **admin** | devices, gaps, versions, seats, licence | activity rows |
| **auditor** | activity — only inside an approved window | anything without a countersignature |
| **countersigner** | every request, with its reason; the whole access log | rows, unless also an auditor |

```console
$ cyberbrain hub principal add "M. Kraus" --role auditor
$ cyberbrain hub principal add "Works council" --role countersigner
```

Credentials are shown once and stored as a hash, like device tokens. Granting a role is
itself an entry in the hub's log.

### One request, two people

```console
# the auditor asks, naming a reason
$ cyberbrain hub request --device dev_01M1Y9… --reason "Revision query of 5 Sept" --as $AUDITOR
request req_01M1YB… recorded
It gives access to nothing until somebody else countersigns it.

# the works council reads the reason and decides
$ cyberbrain hub approve req_01M1YB… --hours 2 --as $COUNCIL
open until 2026-09-07T18:30:10Z

# only now, and only until the window closes
$ cyberbrain hub disclose req_01M1YB… --out-dir case-2026-09 --as $AUDITOR
14 row(s) from 1 device(s) written to case-2026-09
```

Each refusal on the way says something different, because the fixes differ: reading without
approval, an administrator asking for activity, an auditor trying to countersign, a second
auditor collecting somebody else's approval, and a window that has closed. That last one says
to make a new request rather than extend the old one — so the reason is stated again.

### The record the works council reads

```console
$ cyberbrain hub access-log
… role.granted       hub   {"name":"M. Kraus","role":"auditor",…}
… access.requested   who_… {"reason":"Revision query of 5 Sept","device":"dev_…",…}
… access.approved    who_… {"expires_at":"2026-09-07T18:30:10Z",…}
… access.disclosed   who_… {"request":"req_…","rows":14}

chain holds over 6 entr(ies)
```

The hub keeps its own hash chain for these, separate from the device chains, with the same
append-only triggers. Who looked, why, who approved it, and whether anyone removed that
afterwards — all four have an answer.

### What this does not do

It enforces the **route**. Through this program, activity is unreachable without a request
somebody else approved, inside a window that closes itself, and every step is recorded.

It does **not** defend against someone with file access to the hub's database: they can open
it with any SQLite tool, and reading leaves no trace anywhere. That is why the hub belongs on
a machine with controlled access — and why a works agreement should describe this as a
procedure supported by software, not as a guarantee made by it. The chain does cover the
other half: rows cannot be changed or removed without it showing.

## Checking what the hub holds

```console
$ cyberbrain hub verify --data /var/lib/cyberbrain/hub.db
laptop-anna              chain holds over 12480 row(s)
ws-007                   chain holds over 15902 row(s)

28382 row(s) checked; everything the hub holds is as it arrived
```

Each delivery was checked as it arrived, so this asks a different question: is what is on
disk *now* still what arrived? That is what a restored backup, a disk fault or a helpful
administrator raises, and the append-only triggers do not answer it — they stop the database
being *asked* to change, not being replaced. Exits non-zero on a broken chain, so a nightly
job is one line.

## A report for a period

```console
$ cyberbrain hub report --from 2026-01-01T00:00:00Z --to 2026-03-31T23:59:59Z \
    --out-dir q1-2026 --data /var/lib/cyberbrain/hub.db
```

A directory, not a file: a bundle is one chain and the hub holds one per device, so merging
them would produce something that verifies as nothing. Each device gets a `.jsonl` in the
format from [`docs/AUDIT-EXPORT.md`](AUDIT-EXPORT.md), plus a `summary.txt` naming what is in
each one.

Every file verifies on its own, with this program or without it:

```console
$ cyberbrain verify-export q1-2026/dev_01M1Y9….jsonl
$ python3 scripts/verify-audit-export.py q1-2026/dev_01M1Y9….jsonl
```

**A device with nothing in the period still gets a file.** "This machine did nothing that
week" is a finding, and its absence would read as an oversight.

## What is not in this slice

- **No certificate of its own.** The hub serves the certificate you give it (see
  *Encrypting it*) and it does not make one. In a network with no certificate authority of
  its own that leaves a self-signed certificate and a browser warning, or plain text. The
  slice that generates one and pins it at enrolment is the next one.
