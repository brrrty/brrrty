#!/bin/sh
# Smoke-tests a release binary: its version, a fixture build that must match
# the fixture's expected output, and a failing build that must exit 1.
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

echo "smoke: $bin ($version) passed"
