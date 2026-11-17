#!/bin/sh
# Smoke-tests a release binary: its version, a fixture build that must match
# the fixture's expected output, a failing build that must exit 1, and the
# date of a page without one.
#
#   .github/scripts/smoke-binary.sh BINARY VERSION
#
# POSIX sh, so it runs in Alpine too. Run from the repository root.
set -eu

bin=$1
version=$2
case $bin in /*) ;; *) bin=$PWD/$bin ;; esac

said=$("$bin" --version)
if [ "$said" != "brrrty $version" ]; then
  echo "smoke: $bin --version said '$said', expected 'brrrty $version'" >&2
  exit 1
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cp -R tests/fixtures/basic/site "$work/site"
(cd "$work/site" && "$bin" build --input src --passthrough src/images --output "$work/out" --quiet)
diff -r tests/fixtures/basic/expected "$work/out"

mkdir -p "$work/broken"
printf '{%% if %%}\n' > "$work/broken/index.liquid"
status=0
(cd "$work/broken" && "$bin" build --output "$work/broken-out" --quiet) 2>/dev/null || status=$?
if [ "$status" -ne 1 ]; then
  echo "smoke: a failing build exited $status, expected 1" >&2
  exit 1
fi

# A page without a date is dated by its creation time (11ty's
# stat.birthtime), not its modification time. Moving mtime forward leaves the
# creation time alone everywhere (on APFS an earlier mtime pulls it back);
# BSD and busybox touch both take -t.
mkdir -p "$work/born"
printf '%s\n' '---' 'permalink: /date.txt' '---' '{{ page.date | dump | safe }}' > "$work/born/index.njk"
touch -t 203001020304 "$work/born/index.njk"
(cd "$work/born" && "$bin" build --output "$work/born-out" --quiet)
if grep -q '"2030-' "$work/born-out/date.txt"; then
  echo "smoke: a page without a date got its modification time ($(cat "$work/born-out/date.txt")), expected its creation time" >&2
  exit 1
fi

echo "smoke: $bin ($version) passed"
