# Contributing

Thank you for looking. Two things are worth knowing before you write any code, because both
are load-bearing and neither is negotiable.

## 1. The clean room

Cyberbrain is an original work. It is not derived from, and shares no source code with,
`openwolf` / `openwolf-enhanced` (AGPL-3.0-only, © Cytostack Pvt Ltd) or `cfetch`
(FSL-1.1-ALv2, © Corbet Labs). Both were operated in production by the author, and the
*operational lessons* from that inform the specification; lessons about a problem domain are
not protected expression, source code is.

So, from [`docs/SPEC.md`](docs/SPEC.md) §0, and it binds you the moment you open a pull
request:

> **No contributor to Cyberbrain may consult the source of either project.** Behaviour
> described here is specified from first principles and must be implemented from this
> document alone.
>
> Any implementer who has read the source of the above projects must declare it and step
> back from the affected module.

In practice:

- Do not open those repositories to "see how they did it", not even to compare an algorithm.
- If you already know one of them from elsewhere, say so in the pull request and stay away
  from the module it touches. Nobody will think less of you; it is the only thing that keeps
  the provenance claim in the README true.
- Found a bug in one of them while using it? Report it to them. Do not port their fix here.

**None of this applies to you as a user.** Run Cyberbrain next to whatever else you like,
including the two projects named above; the licence restricts building a competing product,
not using one. The rule binds people who write code that ends up in this repository, and it
binds them because of what the repository claims about itself.

This is not lawyer theatre. The licence, the ability to dual-licence and the sentence "shares
no source code with any other memory tool" all rest on it, and any one contribution that
breaks it breaks all three retroactively.

## 2. What a good change looks like here

**Calibrate the test against the broken state first.** A test that has never failed proves
nothing. Write it, run it against the code *without* your fix, watch it fail, then fix. Where
the change is about behaviour a user sees, say in the commit message what the failure looked
like. Ring 0 of the author's own store says it shorter: never call a check passed without
running it against the broken state first.

**Measure claims, do not argue them.** "This is faster" needs two numbers. There is a
retrieval evaluation harness in [`scripts/recall-eval.py`](scripts/recall-eval.py) for changes
to search quality; a change that cannot be measured with it should say why.

**Comments say why, not what.** The code says what it does. The comment exists for the
decision behind it, the thing that was tried and did not work, or the trap the next reader is
walking into.

**Caveats are a feature.** Anything that makes a result look more certain than it is will be
rejected, however convenient. If a check was skipped, the result says so.

## Build and test

```console
$ (cd ui && npm ci && npm run build)      # the embedded page; needs a node toolchain
$ cargo build --release
$ cargo test --workspace
$ cargo test --release -- --ignored       # the benchmarks: hooks, recall, scan
$ cargo fmt --all --check
$ cargo clippy --workspace --all-targets -- -D warnings
$ cargo deny check
```

Without a node toolchain, `--no-default-features` leaves the page out and everything else
works. The web UI has its own checks: `npm run typecheck` and `npm run licenses`.

## Things CI will fail you for, by design

- An HTTP client constructed outside the egress module. Every outbound path is registered at
  compile time (SPEC §12.1); a new one is a deliberate act, not an import.
- A dependency that drags in a C toolchain or a system library. The shipped binary needs
  nothing at runtime.
- `clippy` warnings, unformatted code, a yanked or non-permissive dependency.
- On Windows: a hook that does not survive `cmd.exe` and a path with a space in it.

## Things that will not be accepted

Telemetry, usage pings, crash reporters, "anonymous" statistics. The egress register has two
entries and the compliance screen ends with the sentence "Telemetry does not exist." That is a
promise to the reader, not a default someone gets to flip.

## Licensing of your contribution

The project is [FSL-1.1-ALv2](LICENSE.md), © Krynex Labs. By opening a pull request you state
that the work is yours to give and that it may be distributed under that licence.

Sign your commits off with `git commit -s` ([Developer Certificate of
Origin](https://developercertificate.org/)). It is one line and it is the record that you
wrote what you sent.

## Security

Please do not open a public issue for a vulnerability. [`SECURITY.md`](SECURITY.md) has the
way in.
