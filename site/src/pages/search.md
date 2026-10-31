---
layout: ../layouts/Doc.astro
title: "Search and models"
description: "Lexical search out of the box, semantic search with a static embedding model you place yourself, a transformer through a local Ollama, the optional inference endpoint for contradictions, and local speech to text."
slug: "search/"
---

# Search and models

Out of the box, search is lexical, and every result says so in a caveat. Lexical means literal: without a model, `postgres` does not find `PostgreSQL`. Semantic search needs a model, and **nothing is ever downloaded on your behalf**. Every model on this page is one you put there.

## Static embeddings: a laptop is enough

The default backend is `model2vec`: static embeddings, which are a table lookup and an average rather than a forward pass through a network. One CPU core is enough. The artefact on disk is larger than the work it does, because what takes the space is vocabulary.

Three steps, all yours:

```console
$ # 1. put model.safetensors and tokenizer.json under .cyberbrain/models/model2vec,
$ #    for example minishlab/potion-multilingual-128M (~500 MB) or potion-base-8M (~30 MB), both MIT
$ cyberbrain manifest   # 2. records the blake3 digest of each file
$ cyberbrain scan       # 3. vectors are written when a note is indexed
```

The manifest is the point. An artefact that does not match the recorded digests is refused rather than loaded, because a model swapped underneath a store changes what its vectors mean without changing anything you can see. If the stored vectors belong to another profile, semantic search is switched off loudly, not mixed.

What it costs, from the README of 0.8.4:

| | without a model | with the embedding model |
|---|---|---|
| one search | 10 ms | 2.3 s, most of it loading the model |
| memory | 14 MB | 1.55 GB peak while the search runs |
| full re-index | 0.7 s | 4.0 s |
| on disk | — | 507 MB for the artefact |

<sub>Measured on 2026-09-06 against a store of 1,004 notes and 4,031 blocks, on a server without a GPU (AMD EPYC-Milan, 12 vCPU, 23 GB RAM). Your corpus will give other numbers.</sub>

On Linux and macOS, `recall` starts a small daemon that keeps the model loaded and answers later searches over a socket only your account can open, so the loading cost is paid once rather than per query. It leaves when idle and as soon as the binary, `cyberbrain.toml`, your `waivers.toml` or the model manifest changes. `CYBERBRAIN_NO_DAEMON=1` turns it off, and on Windows every search loads the model itself. `cyberbrain status` shows the daemon on its `daemon:` line.

## A transformer through a local Ollama

Static embeddings know no context and no word order. With [Ollama](https://ollama.com) on the machine, a transformer embedding model finds noticeably more. The README of 0.8.4 reports, for 34 questions over 1,800 German project notes: the right note in the top three for 79 % of them with potion-multilingual-128M and for 88 % with `embeddinggemma:300m-qat-q4_0` (245 MB). The price is about 20 ms per query instead of a few, and a full re-index takes minutes instead of seconds.

```console
$ ollama pull embeddinggemma:300m-qat-q4_0   # you download it
$ cyberbrain bench-embedding                 # measures what is installed on this machine
```

`bench-embedding` times every installed candidate on a sample of your own blocks and prints the `[embedding]` section for the one that fits (`backend = "ollama"` and an `[embedding.ollama]` table). That section pins the digest of the weights Ollama loads. It plays the manifest's part: if the weights behind a tag change, Cyberbrain refuses them instead of mixing their vectors into the index.

The Ollama address goes through the same check as every inference endpoint (loopback or private range), each request is an audit row, and no hook ever calls it. If Ollama does not answer, recall falls back to lexical and says so; a scan indexes the note without vectors and the next scan fills them in.

## The inference endpoint

The optional language-model layer talks to any OpenAI-compatible endpoint: Ollama, LM Studio, llama.cpp, vLLM, llamafile, NVIDIA PAIR. It checks hits for contradictions, summarises a session (`propose-session`) and suggests ring and tags. Every core function works without it.

```toml
[inference]
base_url = "http://127.0.0.1:11434/v1"
model = "qwen3:8b"            # without a model the endpoint is never contacted
timeout_ms = 30000
contradiction_budget_ms = 3000
```

Rules that do not bend:

- The address must resolve to loopback or a private range. A public address, or `100.64.0.0/10` (carrier-grade NAT, also what Tailscale uses), needs a waiver that you grant yourself; see [`cyberbrain trust`](../compliance/#a-cloned-repository-cannot-send-your-notes-away).
- Model, endpoint and token counts of every call go into the audit log. Many local servers send no token counts; then the log says they were absent and never estimates one.
- The model may never propose ring 0 or 1. A suggestion of 0 or 1 is demoted to 2 and marked.
- An unreachable endpoint degrades the feature and says so. It never falls back to a remote provider.

The README notes that on a CPU-only server one contradiction check took 126 s, far over the 3 s budget; the hits came back unchecked and said so. This layer is the one part that wants better hardware.

### One machine for the team

The endpoint may sit on another machine in your network, so one box with a GPU can serve everyone. Set `base_url` to its private address in each store. [`docs/SHARED-INFERENCE.md`](https://github.com/bassprofressor-lab/cyberbrain/blob/main/docs/SHARED-INFERENCE.md) has the setup. The serving machine has no authentication in front of it, which is exactly why the address has to stay private.

## Speech to text

`cyberbrain transcribe <file>` sends a recording to a [whisper.cpp](https://github.com/ggml-org/whisper.cpp) server on your own machine and writes the text the way `write` does: PII check, audit row, index. In the web page, a microphone button beside the search field and the note editor puts dictated text into the field; nothing is saved until you save it.

```toml
[transcription]
base_url = "http://127.0.0.1:8178"   # whisper-server --convert
language = "en"                      # default "de"; any whisper code or "auto"
```

It is off until `base_url` is set, and held to the same address rule as inference. No C++ is in the binary: the model runs in its own process.
