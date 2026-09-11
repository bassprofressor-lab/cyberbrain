# Rolling Cyberbrain out to a company

One collector, forty workstations, and nobody on those workstations opening a command
prompt. This is the order that works, with what each step is for and what it does not do.
Everything referred to here is described in more detail in [`HUB.md`](HUB.md).

What is tested and what is not is said where it matters: the installer switches, installing
over a running installation, and enrolment with a fleet invitation are exercised in CI on
Windows. Group policy and Intune themselves are not, because CI has neither; the commands
they run are the tested ones.

## 1. The collector

On the one machine that collects, as an administrator:

```console
> cyberbrain-setup.exe /S /HUB
```

That installs the program, registers the hub as a Windows service on port 7788, opens that
port in the firewall, makes the hub a certificate of its own, and creates
`C:\ProgramData\Cyberbrain` for its record. Then:

1. Put the licence file you were sent into `C:\ProgramData\Cyberbrain\licence.txt` and restart
   the service. `cyberbrain hub licence show --data C:\ProgramData\Cyberbrain\hub.db` says
   what it found. Seats are machines; see *Seats are machines* in `HUB.md`.
2. Open the hub's page on that machine and set the administrator password.
3. Register the people the two-person rule needs, at least one countersigner:
   `cyberbrain hub principal add Betriebsrat --role countersigner --data …`. The credential is
   printed once.
4. Set a retention period if the works agreement has one: `cyberbrain hub retention set P2Y`.
   Setting it removes nothing; see *How long it keeps things*.
5. Schedule a nightly `cyberbrain hub backup <dated file> --data …` onto storage that is backed
   up. It exits non-zero when the copy does not verify, so a failed backup is a failed task.

## 2. One invitation for the rollout

```console
> cyberbrain hub invite create --uses 60 --expires P14D --label "Rollout September" ^
    --hub-url https://hub01.firma.local:7788 --out \\fileserver\rollout\rollout.json ^
    --data C:\ProgramData\Cyberbrain\hub.db
```

- `--uses` counts projects, not machines: every project a person opens is a device of its own.
  Give it room.
- `--hub-url` is the address the workstations reach the hub at, as they will type it. When the
  hub serves with a certificate of its own, which is what `/HUB` sets up, the invitation
  carries its fingerprint, and the workstations need nothing in their certificate store to
  deliver.
- The file is a credential for every enrolment it has left. Put it on a share only the rollout
  can read, and keep `--expires` short; the most it allows is 90 days.

## 3. The workstations

The same installer, without `/HUB`, with the invitation:

```console
cyberbrain-setup.exe /S /INVITE=\\fileserver\rollout\rollout.json
```

It runs as an administrator, which is what a computer startup script under group policy, an
Intune Win32 app and an SCCM application all do. It installs the program and the launcher,
puts the program on the PATH, and copies the invitation to
`C:\ProgramData\Cyberbrain\fleet-invitation.json`. If the invitation cannot be copied, the
installer exits with 2, so the deployment tool reports a failure instead of a machine that
looks set up and never connects.

There is no MSI. Intune and SCCM deploy an EXE with these switches as they are; group policy's
*Software installation* only takes MSI packages, so under group policy use a computer startup
script that runs the line above.

`fleet-invitation.json` in `C:\ProgramData` can be read by every account on the machine. That
is what lets the launcher, running as the person, use it; it is also why the invitation should
be withdrawn once the rollout is done (step 5).

## 4. What a person sees

Nothing, until they open a project in the launcher. The first delivery the launcher attempts
for that project shows it is not connected, and then it asks, once:

> Your company has set this computer up to connect projects to its hub.
> Connect "Angebote"? It will deliver its audit trail: what happened, not what a note says.
> Sharing notes is a separate setting, and it stays off.

Yes enrols that project with the invitation: the machine's name and the project folder's name
go to the hub, and the hub makes the device `ws-021/Angebote`. No is remembered for that
project and not asked again; the project's menu still has **Connect to the company hub…**.
Nothing is connected without an answer, because a project on a company machine can still be
somebody's own.

Without the launcher, the same thing from a prompt, inside the project:

```console
> cyberbrain hub enrol C:\ProgramData\Cyberbrain\fleet-invitation.json
```

## 5. Checking, and closing the rollout

On the collector:

```console
> cyberbrain hub fleet --data C:\ProgramData\Cyberbrain\hub.db
> cyberbrain hub invite list --data C:\ProgramData\Cyberbrain\hub.db
```

`hub fleet` lists every device with its machine, when it was last heard from, and what is
wrong with it, including clients on an older version than the hub. When the rollout is done:

```console
> cyberbrain hub invite revoke ec_01M2… --data C:\ProgramData\Cyberbrain\hub.db
```

Devices it enrolled stay; nobody further gets in with it. Remove
`C:\ProgramData\Cyberbrain\fleet-invitation.json` from the workstations with the same tool that
put it there; uninstalling Cyberbrain removes it too.

## 6. Updates

An update is the new installer run the same way over the old one, with the same switches. It
asks a running launcher to close, stops the hub service where there is one, replaces the
files, and starts the service again; CI checks the service comes back. Nothing updates itself:
there is no update service and no call home, so updates are rolled out like the installation
was. The fleet view names the machines still on an older version.

Update the collector before the workstations when a release says so. Two in the current
changelog do: names with umlauts, and the per-device tokens.
