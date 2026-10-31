---
layout: ../layouts/Doc.astro
title: "Team hub"
description: "One machine collects the others' audit trail: licence and seats, running it on Linux or as a Windows service, TLS with a pinned certificate, enrolment, note sharing by bereich with a countersignature, roles, backups, retention and alerts."
slug: "hub/"
---

# Team hub

A team keeps its notes where it always did, in each person's store. A hub is the one machine that collects the **audit trail**: which machine did what kind of thing, and when, in chains that cannot be edited afterwards without it showing. A row says that a note was written, never what it said.

A hub is a separate surface, not `serve` opened to the network. It authenticates, and what it can do is short: take audit rows and, where a store switched it on and two people agreed to a bereich, hold and hand back the notes of that bereich. It cannot read, write, delete or apply retention on anybody's store.

This page condenses [`docs/HUB.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/HUB.md), which has the full picture. Commands and flags here are checked against 0.8.4.

## The licence

A hub needs one; without it nothing is registered and nothing collected. It is a signed file checked against a key built into the binary. There is no call home.

```console
$ cyberbrain hub licence install licence.txt --data /var/lib/cyberbrain/hub.db
$ cyberbrain hub licence show --data /var/lib/cyberbrain/hub.db
```

`licence show` exits non-zero when the hub is not collecting, so a monitoring check is one line. A licence file dropped beside the record (`licence.txt`) is picked up at start.

**Seats are machines.** Every project is its own store and so its own device on the hub, but devices that report the same machine share one seat. A second project on a machine that already has a seat can be registered on a full licence by naming the machine: `hub add ws-021-dispo --machine ws-021`.

**When it expires** the hub stops accepting rows and does nothing else. The record stays readable, devices keep working, clients buffer, and a renewed licence takes what they held without a gap in the chain.

## Running one

On Linux:

```console
$ cyberbrain hub add "laptop-anna" --data /var/lib/cyberbrain/hub.db
$ cyberbrain hub serve --addr 0.0.0.0:7788 --data /var/lib/cyberbrain/hub.db --tls-generate
```

`hub add` prints the device token once and stores only a hash. `--invite <file> --hub-url <url>` writes an invitation file instead, which carries everything the machine needs (optionally the shared inference address with `--inference-url`). Hand it over like a password. `hub serve` defaults to `127.0.0.1:7788`; a hub only its own machine can reach collects nothing, so give it an address the network reaches. HUB.md has a systemd unit.

On Windows, tick **Hub service (collector)** in the installer, or run `cyberbrain-setup.exe /S /HUB`. That registers the service *Cyberbrain Hub* on `0.0.0.0:7788`, opens the port in Windows Firewall, makes the hub its own certificate and creates `C:\ProgramData\Cyberbrain`. Licensing it is copying `licence.txt` there and restarting the service. The same by hand:

```console
> cyberbrain hub service install --data C:\ProgramData\Cyberbrain\hub.db
> cyberbrain hub service status
```

`hub service uninstall`, and removing the program, leave the record alone.

### Many machines at once

A fleet invitation enrols many projects with one file:

```console
$ cyberbrain hub invite create --uses 60 --expires P14D --label "Rollout Disposition" \
    --hub-url https://hub.example.internal:7788 --out rollout.json --data /var/lib/cyberbrain/hub.db
```

Each project that runs `cyberbrain hub enrol rollout.json` (or *Connect to the company hub…* in the Windows launcher) gets a device of its own. `--expires` is at most 90 days; `hub invite list` and `hub invite revoke` manage them. An address that tries ten unknown codes within ten minutes is turned away with `429` for the rest of those minutes. Behind a reverse proxy every machine has the proxy's address, so one guesser there locks out everybody for that time. The company rollout is in [`docs/ROLLOUT.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/ROLLOUT.md).

## Encryption

Without a certificate authority of your own, `--tls-generate` makes a certificate once and keeps it beside the record. Every invitation issued afterwards carries its fingerprint, and enrolled machines **pin** it: they accept that certificate and nothing else, and refuse plain http. Browsers do not know it and warn; `hub cert show` prints the fingerprint and the commands to put it into a machine's trust store, `hub cert export <file>` copies the certificate (never the key) for a share or a group policy.

With a certificate of your own, give `--tls-cert` and `--tls-key` in PEM. Such a certificate is never pinned; clients verify it against the machine's trust store, so a renewal does not stop them.

Without any certificate the hub still collects, warns at every start and shows a banner. The administrator password can then only be typed at the hub's own machine.

## The hub's page and signing in

The hub's page is at `http://localhost:7788/` on its own machine: licence, seats, devices, people with a role, fleet invitations. It is rendered on the server and has no JavaScript.

There is no default password. The account is `admin`, and the **first visit from the machine the hub runs on** sets the password. After that the page opens from any desk, provided the hub is encrypted. `hub admin reset --data …` forgets the password, and the next visit from the hub's own machine sets a new one. A wrong password costs a fixed delay rather than a lockout, and every refused sign-in is in the hub's own log.

"From the machine the hub runs on" means a loopback connection whose `Host` header also names loopback. That keeps a DNS-rebinding page from claiming an unclaimed hub.

> **Open point in 0.8.4: a reverse proxy and the first sign-in.** A reverse proxy on the hub's own machine connects from loopback, and depending on the `Host` it forwards, the first-visit rule above can misjudge whether a request comes from the machine. How the hub should treat that case is not settled yet. Until it is, set the administrator password directly at the machine, at `http://localhost:7788/` and not through the proxy, before you put a proxy in front, and let the proxy pass the original `Host`.

## Delivering to it

```console
$ cyberbrain hub enrol ws-021.json
$ cyberbrain hub push
```

`hub push` sends the store's audit rows to the hub it was enrolled with. Run it on a timer, once an hour is plenty; the Windows launcher delivers by itself every quarter of an hour. Nothing is buffered separately: the audit log is the buffer, the hub skips what it already has, and a delivery that leaves a gap is refused with `409`. Overlap is fine.

The device token is not in the store. It goes into your configuration directory, one file per store and hub, or into `CYBERBRAIN_HUB_TOKEN` for a service account. Editing the hub URL in `cyberbrain.toml` does not redirect the audit trail: the egress gate refuses any destination other than the hub this store enrolled with.

`hub fleet` shows the devices, those with something wrong first: never reported, a refused delivery, quiet for 71 hours, an older version.

## Sharing notes

By default the hub holds audit rows and nothing else. A note's text travels only if **all** of these hold:

| | |
|---|---|
| `allow_note_sync = true` in the store's `[hub]` section | off by default |
| the note carries a `bereich` | no bereich, no sharing |
| ring 2, 3 or 4 | rings 0 and 1 never leave the machine, whatever is granted |
| a grant for that device and bereich, in the right direction | `send`, `receive` or `both` |
| a second person countersigned the grant | not the one who wrote it |

```console
$ cyberbrain hub grant add --device dev_01J… --bereich disposition --direction both \
    --reason "Shift handover within the department"
$ cyberbrain hub grant approve bg_01J… --as <countersigner credential>
```

Two people, because whoever runs the hub registers devices and can read a device token out of an invitation file. If that person could also point a bereich at a device, every note text on the hub would be one form away from them.

On the machine, `cyberbrain hub push --notes` sends the notes, and `cyberbrain hub pull` takes what the hub holds for this machine. `pull` never overwrites a note changed here since the last pull; it reports it. Erasures are reported and acted on only with `--apply-erasures`. Two machines that changed the same note produce a conflict, which an `editor` of that bereich settles with `hub conflicts`.

`cyberbrain hub erase <name> --bereich <b>` removes the hub's copy and leaves a tombstone. It works even with sharing switched off. A local `forget` does not reach the hub; it says so and names this command.

## Who may see what

| Role | Sees | Does not see |
|---|---|---|
| `admin` | devices, gaps, versions, seats, licence | activity rows |
| `auditor` | activity, only inside an approved window | anything without a countersignature |
| `countersigner` | every request with its reason, the access log, grants waiting | rows, unless also an auditor |
| `editor` | note conflicts in the bereiche assigned to them | anything else |

```console
$ cyberbrain hub principal add "M. Kraus" --role auditor --expires P1Y
$ cyberbrain hub request --device dev_01M1Y9… --reason "Revision query of 5 Sept" --as $AUDITOR
$ cyberbrain hub approve req_01M1YB… --hours 2 --as $COUNCIL
$ cyberbrain hub disclose req_01M1YB… --out-dir case-2026-09 --as $AUDITOR
```

Credentials are shown once and stored as a hash. Every step is in the hub's own hash-chained log, `hub access-log`. Countersigners and administrators are made only at the machine with `hub principal add`, never from the page.

**What this does not do.** It enforces the route: through this program, activity is unreachable without a request somebody else approved. It does not stop someone with file access to the hub's database, who can open it with any SQLite tool. Put the hub on a machine with controlled access, and describe this in a works agreement as a procedure supported by software, not a guarantee made by it.

## Keeping it

| Command | |
|---|---|
| `hub verify` | re-check every chain against the rows on disk now; non-zero on a broken chain |
| `hub backup <file>` | a restorable copy through SQLite, checked after writing; never overwrites. Do not copy `hub.db` by hand: the newest rows may still sit in `hub.db-wal`. |
| `hub retention set P2Y` | the period; removes nothing by itself |
| `hub retention propose --reason …` / `approve <id>` | a purge, written by one person and carried out when a countersigner signs |
| `hub alerts set <url>` | post what needs a person to one webhook: countersignatures waiting, the licence ending, devices quiet for 48 hours, failing governance calls, used decoys. Loopback or private unless `--allow-public` |
| `hub report --out-dir <dir>` | what the fleet did in a period, as state: one line per device with its row count and whether its chain holds. Not the rows; those take `request`, `approve` and `disclose`. |
| `hub revoke <id>` | stop a device from sending; its rows stay |

Backups keep what a purge later removes. If the retention period is a promise to the people whose activity this is, rotate the backups within the same period.
