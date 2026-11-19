#!/bin/sh
# Times 11ty 3.1.8 on one generated site inside Docker (node:22), with
# `npm ci` run in the container, never on the host. The site folder is
# mounted read-only and copied in; the last build's output is copied back
# to <out-folder> for comparison with Brrrty's.
#
#   bench/jsdata/eleventy-docker.sh <site-folder> <out-folder> [runs]
set -eu
site=$(cd "$1" && pwd)
mkdir -p "$2"
out=$(cd "$2" && pwd)
runs=${3:-3}
docker run --rm -v "$site:/site:ro" -v "$out:/out" -e RUNS="$runs" node:22 sh -c '
  set -e
  cp -r /site /work && cd /work
  npm ci --force --no-audit --no-fund --loglevel=error >/dev/null
  node --version
  node -e "
    const { spawnSync } = require(\"node:child_process\");
    const fs = require(\"node:fs\");
    for (let i = 0; i < Number(process.env.RUNS); i++) {
      fs.rmSync(\"_site\", { recursive: true, force: true });
      const t = performance.now();
      const r = spawnSync(process.execPath, [\"node_modules/@11ty/eleventy/cmd.cjs\", \"--quiet\"], { stdio: \"inherit\" });
      if (r.status !== 0) process.exit(1);
      console.log(\"11ty run \" + i + \": \" + ((performance.now() - t) / 1000).toFixed(2) + \" s\");
    }
  "
  rm -rf /out/_site && cp -r _site /out/_site
'
