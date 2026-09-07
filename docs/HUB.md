# The hub: one machine collects what the others did

A team keeps its notes where it always did — on each person's machine, in each person's
store. What the hub collects is the *audit trail*: which machine did what kind of thing,
when, in a chain that cannot be edited afterwards without it showing.

It is a second surface, not `serve` with the address opened up. `serve` binds loopback and
has no authentication, and SPEC §8.2 ties those together deliberately: there is nothing to
authenticate because there is no remote access. It can also read, write, delete and apply
retention — opening its bind would hand that to everyone on the network. The hub
authenticates, and the only thing it can do is take rows.

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

Meant to run as a service — `systemd` on Linux, a Windows service on Windows Server. A hub
that is up only while somebody is logged in makes silence useless as a signal: you could
never tell whether the hub was asleep or the client had stopped.

## Delivering to it

Today, by hand — the client does not send by itself yet. That is the next slice, and the
shape it will use is this one:

```console
$ cyberbrain policy audit --export period.jsonl        # on the client
$ curl -X POST https://hub.example.internal:7788/api/v1/ingest \
    -H "Authorization: Bearer $TOKEN" \
    -H "x-cyberbrain-version: 0.2.1" \
    --data-binary @period.jsonl
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

`cyberbrain hub fleet` shows devices, not activity:

```console
$ cyberbrain hub fleet --data /var/lib/cyberbrain/hub.db
seen           laptop-anna       8 rows  last seen 2026-09-07T15:17:29Z  version 0.2.1
never reported build-01          0 rows  last seen never                 version unknown
revoked        laptop-old       31 rows  last seen 2026-08-02T09:11:02Z  version 0.1.0

8 row(s) in the record
```

Revoking stops a device from sending; its rows stay, because revoking is not a deletion.

## What is not in this slice

- **The client does not send by itself.** Deliveries are made by hand or by a cron entry
  wrapping the two commands above.
- **No licence.** Any registered device may send; seat counting and expiry come with it.
- **No TLS of its own.** Put it behind a reverse proxy inside your network, or wait for the
  slice that gives the hub a certificate and pins it at enrolment. Do not expose it to the
  internet as it stands.
- **Roles are not implemented.** The design has an administrator who sees state and gaps,
  and activity rows only through a two-person request. Today `hub fleet` is state only,
  which is the safe half of that.
