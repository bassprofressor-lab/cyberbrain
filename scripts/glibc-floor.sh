#!/usr/bin/env bash
# Refuse a Linux binary that needs a newer glibc than the oldest system we promise to run on
# (2026-11-16). ubuntu-latest moved to 26.04 between 0.10.3 and 0.10.6, and the 0.10.6 binary
# asked for GLIBC_2.43: it would not start on Ubuntu 24.04, Debian 13 or the project's own server.
#   scripts/glibc-floor.sh <binary> [floor]     floor defaults to 2.39 (Ubuntu 24.04)
set -euo pipefail
bin="$1"; floor="${2:-2.39}"
max="$(objdump -T "$bin" | grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' | sed 's/GLIBC_//' | sort -Vu | tail -1)"
echo "highest glibc symbol in $bin: ${max:-none}, allowed: $floor"
if [ -n "$max" ] && [ "$(printf '%s\n%s\n' "$max" "$floor" | sort -V | tail -1)" != "$floor" ]; then
  echo "::error::$bin needs glibc $max, newer than $floor" >&2
  exit 1
fi
