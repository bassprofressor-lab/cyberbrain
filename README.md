# Cyberbrain

Cited, trust-tiered, local-first memory for AI coding agents.

One native binary. Your notes stay plain Markdown you can read, edit and grep. Nothing leaves
the machine unless you say so, and every path that could is listed in one place you can print.

- **Cited** — every answer resolves back to the exact source block.
- **Trust-tiered** — ring 0 invariants override ring 4 hearsay, and conflicts are reported
  rather than silently resolved.
- **Local-first** — hybrid lexical and semantic search with embeddings computed on your
  machine. Optional local LLM over any OpenAI-compatible endpoint: Ollama, LM Studio, or
  NVIDIA PAIR spreading the work across your RTX box, a DGX Spark and an Apple M4 Mac.
- **Built for the EU** — GDPR erasure and access as commands, an enumerable egress register,
  an append-only audit log, and a model card. Switchable profiles for EU, Switzerland, or off.

Status: **v0.1.0, in development.** See `docs/SPEC.md` for the specification this is built
against.

## Licence

[FSL-1.1-ALv2](LICENSE.md). Free for everything except building a competing product, and it
becomes Apache-2.0 two years after each release. Copyright 2026 Krynex Labs.

Cyberbrain is an original work and shares no code with any other memory tool. See §0 of the
specification.
