# One machine does the thinking for the team

The optional LLM layer — contradiction detection, session summaries, ring and tag
suggestions, rewrites — talks to an OpenAI-compatible endpoint at a base URL you configure.
That URL may be a **loopback address or a private-range address**, which means one machine on
your network can serve everyone else, and no code has to change for it.

This document is the whole setup. It is deliberately small: nothing here is new
functionality, it is a configuration that was always allowed and rarely written down.

## What this is not

- **Not a shared store.** Everyone keeps their own notes, their own index and their own audit
  log. Nothing about the memory itself moves.
- **Not remote access.** `serve` still binds loopback only; the web UI stays personal.
- **Not central logging.** Calls are recorded in *each machine's own* audit log. Collecting
  those centrally is a separate piece of work.

## On the machine that serves

Run any OpenAI-compatible server and bind it to an address the others can reach. With Ollama:

```console
$ OLLAMA_HOST=0.0.0.0:11434 ollama serve
$ ollama pull qwen3:8b
```

The same works for LM Studio, vLLM, llama.cpp's server, llamafile, and NVIDIA PAIR. Open the
port to your own network and to nothing else — there is no authentication in front of it,
which is exactly why the address has to stay private.

## On every machine that uses it

In the store's `.cyberbrain/cyberbrain.toml`:

```toml
[inference]
base_url = "http://192.168.1.50:11434/v1"   # the serving machine
model = "qwen3:8b"                          # unset lets the server pick
timeout_ms = 30000
contradiction_budget_ms = 3000              # how long a recall waits before giving up
```

Two settings you will not need but should know exist. `allow_public_endpoint` is off, and
turning it on means project notes may leave your network — that is what it says, and it means
it. `allow_overlay_network` is its own switch for `100.64.0.0/10`, because that range is
carrier-grade NAT *and* what Tailscale hands out for your own machines; the address alone
cannot tell those apart, so the decision is yours and not a guess.

**The setting lives in the store, not in your account.** Five projects on one laptop means
five files. There is no environment variable for the endpoint today (`CYBERBRAIN_STORE` is
the only one, and it selects the store). For a handful of machines that is a `sed` line; for
a fleet it is the reason the invitation in the hub design carries the address with it.

## Check that it worked

```console
$ cyberbrain status
```

The inference section is the answer. Three states, all of them useful:

| What you see | What it means |
|---|---|
| `reachable`, with the models listed | Done. The configured model appears in the list. |
| `configured but unreachable` | Right address, nothing answering — wrong port, firewall, server down. The reason is printed. |
| Refused at startup | The address is public and `allow_public_endpoint` is off. The message names the host and the setting. |

Verified on 7 September 2026 against a server on a private address: all three states behave
as described, including the refusal, which names the host and points at SPEC §11.

## What it costs you, honestly

- **Nothing on the hot path.** No hook calls this layer synchronously: a hook's budget is
  15 ms and a completion takes seconds. A slow or dead endpoint cannot stall the agent.
- **A recall waits at most `contradiction_budget_ms`.** Over budget, the hits come back with
  a caveat saying the check did not run — never silently.
- **Every call is in the audit log** with model, endpoint and token counts. Where the server
  omits usage numbers, the log records that they were absent and does not estimate them.
- **Embeddings stay local.** They are a table lookup and an average, not a forward pass;
  there is no GPU work to centralise. The peak is memory, on the machine doing the search.

## Windows and Light

The Windows build is the same binary, so the same file and the same settings apply; the
desktop launcher starts that binary and does not manage its configuration. The store's
`cyberbrain.toml` is inside the project folder you pick, not in `%APPDATA%`.

**Cyberbrain Light has none of this.** It depends on `cyberbrain-core` and
`cyberbrain-index` only — there is no LLM layer to point anywhere, and that is by design
rather than by omission.
