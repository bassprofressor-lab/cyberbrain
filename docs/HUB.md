# The hub: one machine collects what the others did

A team keeps its notes where it always did — on each person's machine, in each person's
store. What the hub collects is the *audit trail*: which machine did what kind of thing,
when, in a chain that cannot be edited afterwards without it showing.

It is a second surface, not `serve` with the address opened up. `serve` binds loopback and
has no authentication, and SPEC §8.2 ties those together deliberately: there is nothing to
authenticate because there is no remote access. It can also read, write, delete and apply
retention — opening its bind would hand that to everyone on the network. The hub
authenticates, and what it can do is a short list: take audit rows, and — where a store has
switched it on and two people have agreed to a bereich — hold and hand back the notes of that
bereich. It cannot read, write, delete or apply retention on anybody's store.

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

### Seats are machines

Every project a person opens is its own store, with its own chain, so each one is its own
device on the hub. A seat is a machine: devices that report the same machine share one. Each
delivery carries the machine's name (`COMPUTERNAME` on Windows, the host name on Linux), and
`hub fleet` shows it after the device's name. A device that has not delivered yet has not said
which machine it is on, and counts as a machine of its own until it does.

Counted at enrolment, not at delivery: a device that was allowed to enrol and is then refused
every night looks registered and collects nothing, which is the worst of both. When the
licence is full, a further project on a machine that already has a seat can still be
registered by naming the machine: `hub add ws-021-dispo --machine ws-021`. Revoking frees a
seat once the last device on that machine is revoked; the rows stay, the person left.

The machine name comes from the machine. A company that wanted to could make forty machines
report one name, which is the same trust an offline licence already rests on, and the fleet
view is where it would show.

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

### If you have no certificate

Most companies of this size have no certificate authority of their own, so the hub makes its
own certificate:

```console
$ cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db --tls-generate
cyberbrain hub: https://0.0.0.0:7788/  (record: …)
certificate SHA-256: 6C:D4:7B:43:…  (invitations pin this)
```

On Windows there is nothing to type: `hub service install` does it for you unless you gave it
a certificate or said `--insecure-http`. The pair lands beside the record as `hub-cert.pem`
and `hub-key.pem`, the key owner-only, and it is **made once and then left alone** — every
invitation ever issued names that certificate, so quietly replacing it would stop every
enrolled machine at once. Replacing it deliberately is deleting both files and reissuing the
invitations.

**Every invitation issued afterwards carries its fingerprint, and clients pin it.** A pinned
client accepts that certificate and nothing else — not a company CA, not a public one — and it
refuses to deliver over plain http, where no certificate is presented at all. Nothing has to be
installed on the machines: the invitation they were enrolled with is what tells them what to
expect.

```console
$ cyberbrain hub enrol ws-021.json
enrolled with https://hub.example.internal:7788 as dev_01M1Y9…
token stored at ~/.config/cyberbrain/hub-tokens/4a920a2c….token
this hub is pinned to the certificate 6C:D4:7B:43:…
deliveries go nowhere else, whatever certificate is presented
```

A browser is a different matter: it has never heard of this certificate and will warn. The
fingerprint printed at every start is what you compare the warning against, and the way to stop
being asked is to put the certificate into the machine's trust store. `hub cert` is the command
for both halves of that:

```console
$ cyberbrain hub cert show --data /var/lib/cyberbrain/hub.db
certificate: /var/lib/cyberbrain/hub-cert.pem
SHA-256:     6C:D4:7B:43:…

This is what invitations pin, and enrolled machines need nothing else. A browser is the
exception: it has never heard of this certificate and warns until the machine itself trusts it.

Windows, in an elevated prompt:
  certutil -addstore -f Root /var/lib/cyberbrain/hub-cert.pem
Linux:
  cp /var/lib/cyberbrain/hub-cert.pem /usr/local/share/ca-certificates/cyberbrain-hub.crt && update-ca-certificates
  (the .crt ending is not decoration there; the file is ignored without it)

$ cyberbrain hub cert export \\fileserver\deploy\cyberbrain-hub.pem
```

`export` copies the certificate and nothing else — the key stays where it is. Handing the
certificate around is safe by construction: it is what the hub shows every machine that
connects to it.

### If you have one

Give it a certificate and a key, in PEM, and it serves https with them:

```console
$ cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db \
    --tls-cert /etc/cyberbrain/hub.pem --tls-key /etc/cyberbrain/hub.key
cyberbrain hub: https://0.0.0.0:7788/  (record: …)
certificate SHA-256: 63:4E:3F:E0:…
```

On Windows the same two flags go on `hub service install`, where they become part of the
registration — so an upgrade cannot quietly put the hub back into plain text.

**A certificate you supplied is never pinned**, and clients need nothing: a delivery verifies
against the machine's own trust store, so a certificate from your CA or a public one is trusted
the moment that machine trusts it, by the same rules as everything else on it. Pinning it would
turn its next renewal into every client on the network stopping at once.

What falls between the two is a self-signed certificate made by hand: nobody trusts it and
nothing pins it, so clients will refuse it. Let the hub make its own instead.

**Without a certificate the hub still collects, and says so.** On a network address it logs a
warning at every start and the page carries a banner, because on that hub every device token
crosses the network in the clear. One thing is refused outright: **the administrator password
can then only be typed at the machine the hub runs on.** Signing in from a desk needs https.
The refusal is deliberately narrow — deliveries are unaffected, and a hub that stopped
collecting until somebody produced a certificate would be a worse trade than the one it is
trying to prevent.

Registering a *new* service on a network address is where this is decided, because it is the
one moment a person is standing there: with no flags it makes a certificate and uses it.
`--insecure-http` is how you ask for plain text instead. A hub that is already running is never
stopped over this.

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
`git add .`. It goes into the user's configuration directory instead, one file per store and hub, or
into `CYBERBRAIN_HUB_TOKEN` for a service account. Per store, not per hub: every project a
person opens is its own store with its own chain, so each is its own device on the hub, and
two of them sharing one token would deliver two chains as one device.

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

Two kinds of thing, and they are kept apart on purpose.

**Audit rows, always.** The same rows the client's own audit log holds: timestamp, actor,
action, subject, and the detail as written. No index, no search, no note bodies among them —
a row says that a note was written, not what it said.

**Note text, only where somebody put it there.** A store can also share notes through the
hub, and then the hub does hold the text of those notes. That is off until it is switched on
(`allow_note_sync`, off by default), it never covers rings 0 and 1, it only covers notes
carrying a `bereich`, and each bereich has to be granted to each device and countersigned by
a second person before anything moves. The section below says exactly what that means.

Both halves are in the egress register, and the register is the thing to read rather than
this paragraph: `cyberbrain policy egress` names every way bytes leave a machine, whether
each one carries note content, and whether it is on.

## Sharing notes between machines

An audit trail answers *who did what*. A department that wants the same handover note on
four desks needs something else, and this is it.

**What travels.** A note travels only if all of these hold. Any one of them missing and it
stays where it is, with a refusal that names the missing one.

| | |
|---|---|
| `allow_note_sync = true` in the store's `cyberbrain.toml` | Off by default. A store that never turns it on shares audit rows and nothing else. |
| The note carries a `bereich` | The department, team or domain it belongs to. No bereich, no sharing — the default is private. |
| Ring 2, 3 or 4 | Rings 0 and 1 never leave the machine, and no grant can permit them. Checked on the way out, on the way in, and by a database constraint. |
| A grant for that device and bereich, in the right direction | `send`, `receive` or `both`. Receiving admits foreign content and sending discloses your own; they are separate rights because they are separate risks. |
| Somebody other than the grant's author has countersigned it | See below. |

```console
$ cyberbrain hub grant add --device dev_01J… --bereich disposition \
      --direction both --reason "Schichtübergabe innerhalb der Abteilung"
grant bg_01J… written: device dev_01J… may both bereich disposition
  reason: Schichtübergabe innerhalb der Abteilung

It does nothing yet. A bereich takes two people, so somebody holding a countersigner
credential has to run:
  cyberbrain hub grant approve bg_01J… --as <credential>
```

**Why two people.** Whoever runs the hub registers the devices and can read a device token
out of the invitation file it writes. If that same person could also point a bereich at a
device, then "admin sees state, not content" would be a house rule rather than a property of
the machine: register a device, grant it `receive` on any department, fetch. So a grant is
written by one person and takes effect when another signs it, and the signature is a row in
the hub's log with both names in it. A grant nobody has signed is visible on the hub's page,
marked as waiting, and moves nothing.

**What the hub then holds.** For each shared note: its name, bereich, ring, kind, its
frontmatter as the sender rendered it, and its body. Two machines that changed the same note
without seeing each other produce a conflict, and a conflict row holds the text that was
turned away as well — the same data under a different column name, which is why erasure
clears both.

**Who may read it.** Not the operator. The pages that show note text are the conflict pages,
and they are for the `editor` role and only for the bereiche that editor was assigned.
`cyberbrain hub conflicts` needs the same credential and answers the same way.

**Erasure.** `cyberbrain hub erase <name> --bereich <b>` removes the hub's copy, the text
held in any conflict row for it, and leaves a tombstone so that machines which already
pulled it learn it went rather than merely stopping to see it. It is deliberately not behind
`allow_note_sync`: a setting must not be able to stand between somebody and Art. 17.

**What `forget` does not do.** Erasing a note locally does not reach the hub. `cyberbrain
forget` says so when the note had a bereich and the store is enrolled, and names the command
above. Making a local command delete across the network is a different decision.

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
| **countersigner** | every request, with its reason; the whole access log; bereich grants waiting to be signed | rows, unless also an auditor |
| **editor** | note conflicts in the bereiche they were assigned, both texts | anything outside those bereiche; activity of any kind |

```console
$ cyberbrain hub principal add "M. Kraus" --role auditor
$ cyberbrain hub principal add "Works council" --role countersigner
$ cyberbrain hub principal add "R. Berger" --role editor
$ cyberbrain hub principal assign R. Berger --bereich disposition
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

### The same thing without a command line

Signing in with an auditor or countersigner credential opens `/requests` rather than the
operator's page. An auditor asks there and sees their own requests and nothing else — the
reasons other people wrote are not their business. A countersigner sees every request with
its reason, the bereich grants waiting for a second signature, and the hub's own log, and
approves or countersigns with a button.

This matters more than a convenience. The two roles this hub's promise rests on used to have
a credential, a login box that accepted it, and nowhere to go: their work was reachable only
from a shell on the hub's own machine — the machine whose operator they are there to check.
A works council that has to ask the administrator for a terminal in order to supervise the
administrator is not a supervision anybody should accept.

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

## Backing it up

```console
> cyberbrain hub backup D:\Sicherung\hub-2026-09-11.db --data C:\ProgramData\Cyberbrain\hub.db
written to D:\Sicherung\hub-2026-09-11.db (2211840 bytes)
28382 activity row(s) re-checked in the copy; the hub's own log holds over 41 row(s)
the copy verifies on its own; to restore, stop the hub and put it in place of hub.db
```

**Do not copy `hub.db` instead.** The record runs in WAL mode, so the newest rows can still
sit in `hub.db-wal` beside it. A copy of the one file, taken while the hub runs, can open
cleanly, pass every chain check and hold nothing at all. That is not a hypothetical: it is how
the first version of this command failed its own test. `hub backup` takes a snapshot through
SQLite (`VACUUM INTO`), so deliveries keep arriving while it runs.

Then it opens the file it wrote and checks the copy rather than the original: every device
chain, the hub's own log, and that the copy holds at least the rows and devices the original
held a moment before. Rows are only ever added, so fewer means something is missing. It exits
non-zero when any of that fails, so a scheduled job that runs it turns a broken backup into a
failed job instead of a file nobody opened. It never writes over an existing file; give each
run a dated name, and leave pruning old copies to whatever keeps the backup share.

A copy carries everything the hub holds: every activity row, every shared note text, the
licence. Keep it under the same access rules as the hub itself. Taking one is recorded in the
hub's own log as `hub.backup`, like every other way of reaching those rows.

### Restoring one

1. Stop the hub (`cyberbrain hub service stop` on Windows).
2. Move the current `hub.db` aside, and remove `hub.db-wal` and `hub.db-shm` beside it if
   they are there. They belong to the file being replaced, and SQLite would try to apply them
   to the one you put back.
3. Put the copy in its place as `hub.db`, start the hub, and run `cyberbrain hub verify`.

What the hub's own log recorded after the copy was taken (roles, grants, approvals) is gone
with the file it was in. Activity rows are a different matter: a machine delivers its whole
log unless told otherwise, and the hub takes what it does not have (see *Overlap is fine, a
gap is not*), so rows that arrived after the copy come back from the machines that still
hold them.

## How long it keeps things

Nothing in the hub expires on its own, and nothing is removed without two people.

| What | Kept until | Why |
|---|---|---|
| Activity rows | a purge removes them | the evidence the hub collects, and data about the people at those machines |
| Shared note texts | the note is erased (`hub erase`) | a copy held on behalf of a bereich |
| The text of a conflict | the conflict is resolved | the decision is the record; the version that lost has no further use |
| The hub's own log, requests, grants, people, erasure and purge records | the life of the hub | who decided what, and who looked; removing it would remove the proof that a removal was agreed |

### Purging activity rows

```console
$ cyberbrain hub retention set P2Y --data /var/lib/cyberbrain/hub.db
activity rows are kept for P2Y

$ cyberbrain hub retention propose --reason "Betriebsvereinbarung §7: 24 Monate" --data /var/lib/cyberbrain/hub.db
purge pg_01M2… written: rows older than 2024-09-11T09:00:00Z (P2Y), 18233 row(s) today

$ cyberbrain hub retention approve pg_01M2… --as <countersigner credential> --data /var/lib/cyberbrain/hub.db
pg_01M2… carried out, countersigned by Betriebsrat: 18233 row(s) removed from 12 device(s).
```

Setting a period removes nothing, so a typo cannot delete a year. A purge is written down with
a reason, and carried out when somebody holding a countersigner credential signs it, either
with the command above or with a button on `/requests`. The person who proposed it cannot sign
it. The proposal, the signature, the cutoff and how many rows went from which device all go
into the hub's own log. `hub retention show` says what a purge would remove today, and
`hub retention list` every purge there has been.

The period is days, weeks, months or years (`P90D`, `P18M`, `P2Y`). Hours are refused, and so
is a period of nothing. The cutoff is fixed when the purge is proposed.

**What goes is the start of each chain, never a selection.** Everything before the first row
that is not old enough is removed, and nothing after it. A device whose clock once ran
backwards has an old-looking row after a newer one, and that row stays, because removing it
would cut a hole in the middle of the chain. The hash of the last removed row becomes the
device's floor, and `hub verify` checks what remains from there:

```console
laptop-anna              chain holds over 3120 row(s), from row 9361 (rows before it were purged)
```

A floor proves that the remaining rows are intact and start where they say. It cannot show the
rows that were purged; that is what the purge's entry in the hub's own log is for.

The delete trigger stays in place. It lets a row go only inside the transaction that carries
out a signed purge, and only below the point that purge stopped at for that device. As before,
it does not stop somebody with file access to the database; the chains and the log are what
show that.

**Backups keep what was purged.** A copy taken before a purge still holds those rows. If the
period is a promise to the people whose activity this is, the backups have to be rotated
within the same period.

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

- **No renewal, and no rotation.** The certificate the hub makes itself outlives all of us,
  deliberately: an expiry would stop a hub years from now for a reason nobody is looking for.
  Replacing it means deleting the pair and reissuing every invitation, and nothing here
  automates that. A certificate you supplied renews on your own schedule and the hub does not
  care, which is why that one is never pinned.
- **Nothing puts the certificate into a trust store for you.** `hub cert show` prints the two
  commands that do it, for the two platforms, and `hub cert export` gets the file to where a
  group policy can pick it up — but running them is the administrator's, because writing to a
  machine's root store is not something a collector should do quietly on its way past.
