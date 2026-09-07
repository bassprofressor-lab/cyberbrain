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

Meant to run as a service — `systemd` on Linux, a Windows service on Windows Server. A hub
that is up only while somebody is logged in makes silence useless as a signal: you could
never tell whether the hub was asleep or the client had stopped.

## Delivering to it

On the client, once:

```console
$ cyberbrain hub enrol ws-021.json
enrolled with https://hub.example.internal:7788 as dev_01M1Y9…
token stored at ~/.config/cyberbrain/hub-tokens/4a920a2c….token
inference endpoint set to http://192.168.1.50:11434/v1
```

Then, on a timer — once an hour is plenty:

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

- **No TLS of its own.** Put it behind a reverse proxy inside your network, or wait for the
  slice that gives the hub a certificate and pins it at enrolment. Do not expose it to the
  internet as it stands.
- **Roles are not implemented.** The design has an administrator who sees state and gaps,
  and activity rows only through a two-person request. Today `hub fleet` is state only,
  which is the safe half of that.
