---
layout: ../layouts/Doc.astro
title: "Web page and HTTP API"
description: "cyberbrain serve: the web page and HTTP API built into the binary, on loopback with no login, the terminal in the page, writing rings 0 and 1, and the Windows launcher."
slug: "serve/"
---

# Web page and HTTP API

```console
$ cyberbrain serve
cyberbrain serve: http://127.0.0.1:7777/  (loopback only, this account only; API at /api/v1)
```

The page is compiled into the binary, speaks English and German, and fetches nothing from the network. It covers search, notes with their citations and links, approvals for proposals (with a line diff when a proposal changes an existing note), a graph of the notes by ring, the status of store, model and inference endpoint, the compliance screen with the egress register and the obligation catalogue, and what would be shared with a hub. It also has a command line of its own that runs the same binary against the same store and hands back stdout, stderr and the exit code.

The API is under `/api/v1`, for example `GET /api/v1/status`. The [specification](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/SPEC.md) lists the routes in §8.1.

## Options

| Option | |
|---|---|
| `--port <PORT>` | default 7777 |
| `--no-open` | do not open a browser |
| `--terminal` | offer a real terminal in the page, see below |
| `--allow-resident-writes` | let this server write rings 0 and 1 |

## Why there is no login

`serve` binds loopback and nothing else. It cannot be opened to the network: it reads, writes, deletes and applies retention, and there is nothing to authenticate because there is no remote access. For several machines there is the [hub](../hub/), which is a separate surface with authentication.

Three more boundaries hold this up:

- **Only your own account.** On Linux only your account (and root) can connect.
- **Only under its own name.** Every route answers only under `127.0.0.1:<port>`, `localhost:<port>` and `[::1]:<port>`. Anything else gets `421 Misdirected Request`. That stops DNS rebinding, where a web page resolves its own name to 127.0.0.1 and then reads the API as its own. It also means an `/etc/hosts` alias or a reverse proxy in front does not work, and an SSH tunnel has to forward the same port number.
- **Rings 0 and 1 stay at the terminal.** Without `--allow-resident-writes`, the server does not write them, because any program on this machine, an agent included, could reach it without credentials.

```console
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: evil.example' http://127.0.0.1:7777/
421
```

## The terminal

`cyberbrain serve --terminal` puts a real shell into the page: `ssh`, an agent, anything you would run at a prompt, in the project's directory. It is off unless you ask for it, because every other route reads or writes notes and this one starts a process. With it on, the address printed at start carries a token in its fragment; open that address and no other. A handshake from any other origin is refused.

The egress register lists the terminal as the one path this program does not mediate. What runs in it is yours, in your name, and neither permitted nor recorded by Cyberbrain. It is listed so the register is not read as complete while a shell is one keystroke away.

## Without the page

A binary built with `--no-default-features` has no page. `serve` then answers the API and says the page was not built in.

## On Windows

The [installer](../install/#windows-without-a-terminal) adds a launcher that starts `serve` for a project on a port Windows picks and opens the page, in a window or the browser. It sits in the notification area until you quit it, and quitting stops the server.
