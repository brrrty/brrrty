#!/usr/bin/env bash
# Publish every publishable workspace crate at the workspace version to
# crates.io, skipping any already there, so re-running after a partial
# publish is safe. With --dry-run, packages and builds every crate (none
# skipped) and publishes nothing.
set -euo pipefail
if [ "${1:-}" = --dry-run ]; then
  exec cargo publish --workspace --locked --dry-run
fi
crates=$(cargo metadata --no-deps --format-version 1 --locked |
  python3 -c 'import sys,json
for p in json.load(sys.stdin)["packages"]:
    if p["publish"] is None: print(p["name"], p["version"])')
excludes=()
todo=0
while read -r name version; do
  # crates.io sparse index path: 1/, 2/, 3/<first char>/, or <ab>/<cd>/.
  case ${#name} in
    1) path="1/$name" ;;
    2) path="2/$name" ;;
    3) path="3/${name:0:1}/$name" ;;
    *) path="${name:0:2}/${name:2:2}/$name" ;;
  esac
  if curl -fsS "https://index.crates.io/$path" 2>/dev/null |
      python3 -c 'import sys,json; v=sys.argv[1]; sys.exit(0 if any(json.loads(l)["vers"]==v for l in sys.stdin if l.strip()) else 1)' "$version"; then
    echo "$name $version is already on crates.io, skipping"
    excludes+=(--exclude "$name")
  else
    echo "$name $version will be published"
    todo=$((todo + 1))
  fi
done <<< "$crates"
if [ "$todo" -eq 0 ]; then echo "nothing to publish"; exit 0; fi
cargo publish --workspace --locked "${excludes[@]}"
