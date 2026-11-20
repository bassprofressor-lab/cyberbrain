#!/usr/bin/env bash
# Build and test the products that depend on this core against the core as it is now.
#
#   scripts/check-dependents.sh <core-dir> light <light-checkout>
#   scripts/check-dependents.sh <core-dir> business <business-checkout>
#
# Why (overview of the review 2026-11-19, "Querschnitt"): core 0.10.5/0.10.6 added a required
# field to the public `Frontmatter` and moved the index schema to 7. Cyberbrain Light then
# opened no store the core had touched and `cargo install cyberbrain-light` broke, and
# cyberbrain-business no longer built (E0063). Nobody noticed, because neither was built when
# the core was released. The release workflow runs this before anything is attached to a
# release, and a red run stops it.
#
# Light depends on cyberbrain-core and cyberbrain-index from crates.io, pinned exactly. The
# pin is raised to this core's version and both crates are patched to the checkout, which is
# what Light's own next release would do; nothing else in Light is touched. A break shows up
# as a failed build or test, which is the point.
#
# cyberbrain-business depends on the core by path (`../cyberbrain-p0/crates/...`). The core
# checkout is linked there when that path does not already exist.
set -euo pipefail

core=$(cd "$1" && pwd)
what=$2
dir=$(cd "$3" && pwd)

version=$(cargo metadata --manifest-path "$core/Cargo.toml" --no-deps --format-version 1 |
  python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "cyberbrain-core"))')
echo "core $version at $core"

case "$what" in
light)
  cd "$dir"
  for c in cyberbrain-core cyberbrain-index; do
    if ! grep -Eq "^$c = \"=?[0-9]+\.[0-9]+\.[0-9]+\"" Cargo.toml; then
      echo "::error::Light's Cargo.toml has no plain '$c = \"<version>\"' line; teach this script its new form"
      exit 1
    fi
    sed -i.bak -E "s/^($c = \")=?[0-9]+\.[0-9]+\.[0-9]+(\")/\1=$version\2/" Cargo.toml
  done
  rm -f Cargo.toml.bak
  patch=(
    --config "patch.crates-io.cyberbrain-core.path=\"$core/crates/cyberbrain-core\""
    --config "patch.crates-io.cyberbrain-index.path=\"$core/crates/cyberbrain-index\""
  )
  cargo "${patch[@]}" update -p cyberbrain-core -p cyberbrain-index
  # A patch that does not apply is only a warning to cargo; here it would mean Light was
  # built against crates.io and the check proved nothing.
  cargo "${patch[@]}" tree -i cyberbrain-core --prefix none | head -1 | grep -q "($core/crates/cyberbrain-core)" || {
    echo "::error::cyberbrain-core was not taken from the checkout"
    cargo "${patch[@]}" tree -i cyberbrain-core
    exit 1
  }
  cargo "${patch[@]}" build --all-targets
  cargo "${patch[@]}" test
  ;;
business)
  cd "$dir"
  link="$(dirname "$dir")/cyberbrain-p0"
  if [ ! -e "$link" ]; then
    ln -s "$core" "$link"
  fi
  real=$(cd "$link" && pwd -P)
  if [ "$real" != "$(cd "$core" && pwd -P)" ]; then
    echo "::error::$link exists and is not this core ($real); move it away or check out elsewhere"
    exit 1
  fi
  cargo build --workspace --all-targets
  cargo test --workspace
  ;;
*)
  echo "usage: $0 <core-dir> light|business <checkout>" >&2
  exit 2
  ;;
esac
echo "$what builds and passes its tests against core $version"
