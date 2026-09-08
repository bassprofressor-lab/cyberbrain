# Releasing

The order matters in one place and is easy to get wrong there, so it is written down.

## The one hazard

`default = ["ui"]`, so an ordinary `cargo install cyberbrain` builds the web page in. The
page is not source: it is built by node into `ui/dist` at the repository root, and
`cargo package` takes **nothing** from outside the crate directory — silently, with no
warning. A crate published from a tree where `crates/cyberbrain/ui-dist` is absent carries
no page, and every `cargo install cyberbrain` then fails in `build.rs`.

So: build the page, copy it into the crate, and check that it is in the package listing.

## Order

```console
$ cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings
$ cargo test --workspace                      # and once with --release -- --ignored
$ cargo deny check
$ cargo test --workspace 2>&1 | awk '/^test result: ok\./ {s+=$4} END {print s}'
                                              # the README states a test count; it drifts
                                              # with every change, and a stale one is the
                                              # first number a reader checks. Update
                                              # README.md to match. Sum the counts, do not
                                              # count the lines: `grep -c` answers how many
                                              # test binaries there are, which is 18 and
                                              # was never the number in the README.
$ (cd ui && npm ci && npm run build && npm run licenses)
$ cp -r ui/dist crates/cyberbrain/ui-dist     # the step that cannot be skipped
$ cargo package -p cyberbrain --list --allow-dirty | grep -c '^ui-dist/'  # must not be 0
                                              # --allow-dirty because ui-dist is gitignored:
                                              # without it the command refuses, prints nothing
                                              # to stdout, and the check reads 0 — the same
                                              # answer it gives for the failure it exists to
                                              # catch.
```

Update `CHANGELOG.md`: move `Unreleased` under the new version with the date it actually
shipped. The licence's Apache-2.0 conversion runs from that date, so it is the one line in
this repository that has a legal consequence two years out.

Tagging `vX.Y.Z` runs `.github/workflows/release.yml`, which builds the page and the binary
for Linux and Windows, checks the binary's own version against the tag, runs it once on a
machine that has never seen a store, and attaches the files plus `SHA256SUMS` to a **draft**
release. The notes are written by a person; the workflow only fills the assets.

Then publish the seven crates in dependency order, each waiting for the index to catch up:

```
cyberbrain-core
cyberbrain-index      cyberbrain-embed      cyberbrain-code      cyberbrain-llm
cyberbrain-policy
cyberbrain
```

`crates/cyberbrain/ui-dist` is a build artefact: it is gitignored, listed in `include`, and
should be deleted again after publishing so a stale page cannot be embedded by accident.

## After

Check the published crate the way a stranger gets it:

```console
$ cargo install cyberbrain --root /tmp/cb-check
$ /tmp/cb-check/bin/cyberbrain serve --port 7900        # the page must be there
```

Until the crates are on crates.io, the README says so and gives the from-source path. Keep
those two in step: a README that promises `cargo install cyberbrain` before the crate
exists is the first thing a new reader tries.
