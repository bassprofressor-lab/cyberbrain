---
layout: ../layouts/Doc.astro
title: "Install"
description: "Install Cyberbrain from crates.io, as a checked release binary for Linux, Windows and macOS, or with the Windows installer that adds a tray launcher."
slug: "install/"
---

# Install

The package and the program are both called `cyberbrain`.

## From crates.io

```console
$ cargo install cyberbrain
$ cyberbrain --version
cyberbrain 0.8.4
```

This needs Rust 1.98 or newer. The web page is compiled into the binary and ships inside the crate, so no node toolchain is needed. 0.8.3 on crates.io was published without the page and did not install; 0.8.4 fixes exactly that and nothing else.

If you do not want the page in the binary, leave it out. The command line, the hooks, the MCP server and the HTTP API work the same, and `serve` answers the API while saying the page was not built in:

```console
$ cargo install cyberbrain --no-default-features
```

## As a release binary

Every [release on GitHub](https://github.com/bassprofressor-lab/cyberbrain/releases/latest) carries four files and a `SHA256SUMS`:

| file | for |
|---|---|
| `cyberbrain-linux-x86_64` | Linux, x86_64 |
| `cyberbrain-windows-x86_64.exe` | Windows, x86_64, the bare program |
| `cyberbrain-macos-aarch64` | macOS on Apple silicon |
| `cyberbrain-setup-0.8.4.exe` | the Windows installer, see below |

Check before you run anything:

```console
$ sha256sum -c SHA256SUMS
cyberbrain-linux-x86_64: OK
cyberbrain-macos-aarch64: OK
cyberbrain-setup-0.8.4.exe: OK
cyberbrain-windows-x86_64.exe: OK
$ chmod +x cyberbrain-linux-x86_64
$ ./cyberbrain-linux-x86_64 --version
cyberbrain 0.8.4
```

The binary needs nothing at runtime: no system SQLite, no OpenSSL, no model server, no node.

## Windows without a terminal

`cyberbrain-setup-<version>.exe` installs the command-line tool and a small launcher and puts **Cyberbrain** in the Start menu. The launcher asks once which project to open, starts the server on a port Windows picks, opens the page and stays in the notification area. Right-click it for the project folder, another project, *Start with Windows* (off by default) or Quit. Quitting stops the server. A second start shows the page that is already running instead of a second icon. Launcher and installer speak German on a German Windows and English everywhere else.

The installer is not signed, so SmartScreen warns on first run: *More info*, then *Run anyway*. It puts `C:\Program Files\Cyberbrain` on the machine `PATH`, so `cyberbrain` also works in a new terminal.

For unattended installs the installer takes switches:

| Command | Installs |
|---|---|
| `cyberbrain-setup.exe /S` | a workstation |
| `cyberbrain-setup.exe /S /HUB` | a workstation and the hub service |
| `cyberbrain-setup.exe /S /INVITE=\\server\rollout\rollout.json` | a workstation with a fleet invitation |

`/HUB` sets up the [team hub](../hub/) as a Windows service; `/INVITE` leaves a fleet invitation for the launcher, which asks once per project whether to connect. The step-by-step version for a company is in [`docs/ROLLOUT.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/ROLLOUT.md).

## From a checkout

From the repository the page is built, not packaged, so build it first:

```console
$ git clone https://github.com/bassprofressor-lab/cyberbrain && cd cyberbrain
$ (cd ui && npm ci && npm run build)
$ cargo install --path crates/cyberbrain
```

## After installing

```console
$ cd your-project
$ cyberbrain init                          # creates .cyberbrain/ here
$ cyberbrain install --client claude-code  # writes the hooks into .claude/settings.json
```

Then go on with [five minutes](../quickstart/).

## Uninstalling

```console
$ cyberbrain install --undo     # removes only the entries cyberbrain wrote
$ cargo uninstall cyberbrain
```

The notes stay where they are. `.cyberbrain/notes/` is plain Markdown and belongs to you. On Windows, removing the program leaves a hub's record alone as well.
