"""Calibrate src/order.rs against real 11ty.

usage: ELEVENTY_DIR=/path/to/node_modules/@11ty/eleventy python3 calibrate.py [SITES] [SEED]

Generates SITES random sites (default 80) weighted toward data files, layout
chains, directory-data layouts and `name`/`name.ext` layout spellings
(gen_site.py), records the order 11ty adds their templates to its map and
when (events.mjs), and compares it with the simulation (sim.py, the same
model and step counts as src/order.rs). It also checks the predicted barrier
input: 3 microtasks plus 3 per data file.

"Calibrated" means 0 mismatches over every site 11ty builds: the same
templates, in the same order, each added in the same microtask round.
Sites 11ty refuses (a generated duplicate permalink or layout cycle) are
skipped and counted. Exits 1 on any mismatch.
"""
import os, shutil, subprocess, sys, tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import site_model  # noqa: E402
from sim import simulate  # noqa: E402


def rows_of(text):
    out = []
    for line in text.splitlines():
        p = line.split()
        if len(p) == 4 and p[0].isdigit():
            out.append((int(p[0]), int(p[1]), int(p[2]), p[3]))
    return out


def main():
    if not os.environ.get("ELEVENTY_DIR"):
        sys.exit("Set ELEVENTY_DIR to the @11ty/eleventy package folder.")
    sites = int(sys.argv[1]) if len(sys.argv) > 1 else 80
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 1
    work = tempfile.mkdtemp(prefix="order-calibration-")
    checked = skipped = bad = 0
    try:
        for i in range(sites):
            out = os.path.join(work, f"s{i}")
            subprocess.run([sys.executable, os.path.join(HERE, "gen_site.py"), out, str(seed * 1000 + i)], check=True)
            site = os.path.join(out, "site")
            r = subprocess.run(["node", os.path.join(HERE, "events.mjs"), site], capture_output=True, text=True)
            rows = rows_of(r.stdout)
            if not rows:
                skipped += 1
                continue
            checked += 1
            templates, layouts = site_model.parse(site, rows)
            for t in templates:
                t["order"] = (t["A"], t["start"])
                want_a = 3 + 3 * site_model.data_files(site, t["path"][2:])
                if t["A"] != want_a:
                    bad += 1
                    print(f"s{i}: {t['path']} reached global data at {t['A']}, predicted {want_a}")
            got = simulate(templates, layouts)
            want = [(g, p) for g, a, s, p in rows]
            if got != want:
                bad += 1
                print(f"s{i}: order differs\n  11ty {want}\n  sim  {got}")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    print(f"{bad} mismatches over {checked} sites ({skipped} skipped: 11ty refused them)")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
