#!/bin/sh
# Installs the packed npm packages into a fresh project and runs Brrrty
# through them: the version, a fixture build, a failing build, and
# `brrrty serve` stopped by SIGINT and by SIGTERM through `npm run`.
#
#   npm/test/packages.sh PACKS_DIR VERSION
#
# PACKS_DIR holds the `npm pack` tarballs. Run from the repository root. POSIX
# sh, so it runs in Alpine too; needs node and npm.
set -eu
export NPM_CONFIG_UPDATE_NOTIFIER=false

packs=$(cd "$1" && pwd)
version=$2
repo=$PWD
work=$(mktemp -d)
bare=$(mktemp -d)
trap 'rm -rf "$work" "$bare"' EXIT

fail() {
  echo "packages: FAIL: $*" >&2
  exit 1
}
pass() {
  echo "packages: ok: $*"
}

platform=$(node -p "require('$repo/npm/brrrty/bin/brrrty').PACKAGES[process.platform + '-' + process.arch].slice('@brrrty/'.length)")

cd "$work"
cat > package.json <<'JSON'
{
  "name": "brrrty-package-test",
  "private": true,
  "scripts": {
    "build": "brrrty build --input src --passthrough src/images --output _site --quiet",
    "serve": "brrrty serve --input src --output _serve --port 18080"
  }
}
JSON
cp -R "$repo/tests/fixtures/basic/site/src" src
npm install --save-dev --ignore-scripts --no-audit --no-fund --loglevel=error \
  "$packs/brrrty-$version.tgz" "$packs/brrrty-$platform-$version.tgz"
pass "installed brrrty and @brrrty/$platform"

said=$(npx --no-install brrrty --version)
[ "$said" = "brrrty $version" ] || fail "npx brrrty --version said '$said'"
pass "npx brrrty --version: $said"

npm run --silent build 2>/dev/null
diff -r "$repo/tests/fixtures/basic/expected" _site || fail "the fixture build differs from expected"
pass "npm run build matches the fixture's expected output"

mkdir broken
printf '{%% if %%}\n' > broken/index.liquid
status=0
npx --no-install brrrty build --input broken --output broken-out --quiet 2>/dev/null || status=$?
[ "$status" -eq 1 ] || fail "a failing build exited $status, expected 1"
pass "a failing build exits 1"

# npm passes a signal it gets on to its script shell. dash (Debian's sh)
# keeps running as the binary's parent and does not pass it on, so no program
# run by `npm run` there sees a `kill` aimed at npm; a terminal's Ctrl-C still
# reaches all of them. Use bash for the kill tests below on such systems.
case $(readlink -f /bin/sh 2>/dev/null || true) in
  *dash)
    if command -v bash >/dev/null 2>&1; then
      export npm_config_script_shell=bash
      echo "packages: note: /bin/sh is dash, so npm scripts run under bash here"
    fi
    ;;
esac

# Prints the ids of processes whose command line matches the extended regex
# $1 (pgrep -f where there is one; Debian slim images have none).
find_procs() {
  if command -v pgrep >/dev/null 2>&1; then
    pgrep -f "$1" || true
    return
  fi
  for d in /proc/[0-9]*; do
    [ "${d#/proc/}" != "$$" ] || continue
    tr '\0' ' ' < "$d/cmdline" 2>/dev/null | grep -Eq "$1" && echo "${d#/proc/}"
  done
  true
}

# Fails unless no `brrrty serve` is running within 5 seconds.
no_serve_left() {
  i=0
  while [ -n "$(find_procs '[@]brrrty/.*/bin/brrrty serve')" ]; do
    i=$((i + 1))
    [ "$i" -lt 50 ] || fail "$1 left brrrty serve running"
    sleep 0.1
  done
}

# Starts `npm run serve` in the background and waits for its server line.
start_serve() {
  rm -f serve.log
  npm run serve > serve.log 2>&1 &
  serve_pid=$!
  i=0
  until grep -q 'Server at' serve.log; do
    i=$((i + 1))
    [ "$i" -lt 100 ] || { cat serve.log; fail "brrrty serve did not start"; }
    sleep 0.1
  done
}

start_serve
kill -INT "$serve_pid"
status=0
wait "$serve_pid" || status=$?
[ "$status" -eq 0 ] || { cat serve.log; fail "SIGINT on npm run serve exited $status, expected 0"; }
[ ! -e _serve/.brrrty-serve ] || fail "SIGINT left _serve/.brrrty-serve behind"
diff -r "$repo/tests/fixtures/basic/expected/index.html" _serve/index.html || fail "_serve is not the last good build"
pass "SIGINT on npm run serve stops cleanly and exits 0"

start_serve
kill -TERM "$serve_pid"
status=0
wait "$serve_pid" || status=$?
[ "$status" -ne 0 ] || fail "SIGTERM on npm run serve exited 0, expected a failure status"
[ ! -e _serve/.brrrty-serve ] || fail "SIGTERM left _serve/.brrrty-serve behind"
pass "SIGTERM on npm run serve stops cleanly and exits $status"

# Ctrl-C at a terminal reaches npm, the launcher and (when not kept apart)
# the binary at once, and npm passes it on again: the binary must still see
# one Ctrl-C, stop cleanly and exit 0.
if command -v python3 >/dev/null 2>&1; then
  for cmd in "npm run serve" "npx --no-install brrrty serve --input src --output _serve --port 18080"; do
    # shellcheck disable=SC2086
    python3 "$repo/npm/test/ctrl_c.py" "Server at" $cmd > ctrl-c.log 2>&1 || { cat ctrl-c.log; fail "Ctrl-C on $cmd"; }
    pass "Ctrl-C at a terminal on $cmd exits 0"
  done
  # Ctrl-\ (SIGQUIT) ends everything at once, the binary included.
  python3 "$repo/npm/test/ctrl_c.py" --quit "Server at" npm run serve > ctrl-c.log 2>&1 || { cat ctrl-c.log; fail "Ctrl-\\ on npm run serve"; }
  no_serve_left "Ctrl-\\"
  pass "Ctrl-\\ at a terminal on npm run serve leaves no brrrty serve behind"
else
  echo "packages: skipped: Ctrl-C and Ctrl-\\ at a terminal (needs python3)"
fi

# SIGKILL can't be passed on: the binary notices the launcher is gone and
# stops cleanly by itself.
start_serve
launcher=$(find_procs '[.]bin/brrrty serve' | head -n 1)
[ -n "$launcher" ] || fail "no launcher process found"
kill -KILL "$launcher"
wait "$serve_pid" || true
no_serve_left "SIGKILL to the launcher"
i=0
while [ -e _serve/.brrrty-serve ]; do
  i=$((i + 1))
  [ "$i" -lt 50 ] || { cat serve.log; ls -la _serve/.brrrty-serve; fail "after SIGKILL to the launcher, _serve/.brrrty-serve is left behind"; }
  sleep 0.1
done
pass "SIGKILL to the launcher: brrrty serve stops cleanly by itself"

# Without the platform package, the launcher says which one is missing. (In
# a folder of its own, so Node can't find the one installed above.)
(
  cd "$bare"
  echo '{"name": "no-binary", "private": true}' > package.json
  npm install --save-dev --omit=optional --ignore-scripts --no-audit --no-fund --loglevel=error \
    "$packs/brrrty-$version.tgz" >/dev/null 2>&1
  status=0
  npx --no-install brrrty --version > out.log 2> err.log || status=$?
  [ "$status" -eq 1 ] || fail "without @brrrty/$platform, brrrty exited $status, expected 1"
  grep -q "@brrrty/$platform, which is not installed" err.log || { cat err.log; fail "the missing-package error does not name @brrrty/$platform"; }
  [ ! -s out.log ] || fail "the missing-package error wrote to stdout"
)
pass "without @brrrty/$platform, brrrty exits 1 and names the package"

no_serve_left "The tests"
pass "no brrrty process left running"
