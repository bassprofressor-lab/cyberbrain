---
layout: ../layouts/Doc.astro
title: "Light or full"
description: "When Cyberbrain Light is enough, and when the full Cyberbrain with semantic search, the store guard, proposals, the team hub and the GDPR artefacts fits better."
slug: "light-or-full/"
---

# Light or full

[Cyberbrain Light](https://bassprofressor-lab.github.io/cyberbrain-light/) is a separate, smaller program (`cbl`) for the same store layout. It keeps the rings and the citations and leaves out almost everything else.

|  | [Light](https://bassprofressor-lab.github.io/cyberbrain-light/) | Cyberbrain |
|---|---|---|
| search | keyword (BM25) | keyword **and** meaning, fused |
| model | none | an embedding model you place yourself, or a local Ollama |
| contradiction check | no | against a local inference endpoint |
| Claude Code hooks | session start | session start, store guard before every tool call, prompt, compaction |
| rings 0 and 1 | written at the terminal | written at the terminal; agents propose, a person reviews |
| GDPR and EU AI Act artefacts | no | egress register, erasure with report, subject access, signed audit chain, PII check, obligations |
| team | no | hub for audit trails, optional note sharing |
| web page | no | built into the binary, English and German |
| network access | none, there is no code for it | only the paths in `policy egress`, each off until configured |
| binary (Linux) | 5.0 MB | 20 MB (0.8.4) |

<sub>Light's size is from its own documentation (measured 2026-09-06). Cyberbrain's is the size of the 0.8.4 release file.</sub>

**Take Light** when one person or a small team wants the agent to remember project knowledge, and the notes hold no personal data: build steps, decisions, bugs and their fixes. It runs anywhere and cannot send anything over the network.

**Take the full Cyberbrain** when search has to find meaning and not only words, when an agent works in the project and must not rewrite its own rules, when several machines are involved, or when the store may hold personal data and you have to show what was stored, who wrote it and that a deletion really happened.

> A store can be used by both. Light does not run the full product's checks, though: a note written through Light skips the PII check and the audit row, and `cbl forget` leaves no erasure record. In a store that relies on those, write through `cyberbrain` only.

[Cyberbrain on krynexlabs.de](https://www.krynexlabs.de/en/cyberbrain/)
