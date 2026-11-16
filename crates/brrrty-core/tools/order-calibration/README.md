# 11ty order calibration

`src/order.rs` reproduces the order 11ty adds templates to its template map,
which decides the order of `collections` keys, what a collection pagination
sees, and the order content renders in. It does that by simulating 11ty's
microtask queue while it loads template data, with step counts measured on
11ty itself. These tools measure them again and check the model.

Needs Node and Python 3, and an install of the 11ty version Brrrty targets.

```sh
ELEVENTY_DIR=/path/to/node_modules/@11ty/eleventy \
  python3 crates/brrrty-core/tools/order-calibration/calibrate.py 80 1
```

The arguments are the number of sites and a seed. Add `njk` to mix
Nunjucks templates and layouts into the sites (Brrrty doesn't render them,
but they still take their place in the order):

```sh
ELEVENTY_DIR=... python3 crates/brrrty-core/tools/order-calibration/calibrate.py 80 7 njk
```

**Calibrated** means the last line reads `0 mismatches over N sites`: on
every generated site 11ty builds, the simulation gives the same templates,
in the same order, each added in the same microtask round. The script exits
1 otherwise. The last calibration (11ty 3.1.8) gave 0 mismatches over 74
sites (seed 1, 6 skipped because 11ty refused them), and with `njk` 0 over
74 (seed 7) and 0 over 72 (seed 8).

## Files

- `events.mjs` patches 11ty's classes in-process and prints each
  template's map order and timing. With `-v` it also prints every engine
  and layout promise call and settle: read these to find a new step count.
- `gen_site.py` writes a random site weighted toward data files, layout
  chains, directory-data layouts and `name`/`name.ext` layout spellings.
- `site_model.py` reads a generated site into the simulator's inputs.
- `sim.py` is the simulation, with the same model and constants as
  `src/order.rs`.
- `calibrate.py` runs all of the above and compares.

## Upgrading the 11ty reference version

When `ELEVENTY_VERSION` in `src/build.rs` changes:

1. Rerun the calibration above against the new 11ty, with and without
   `njk`. If it reports
   mismatches, use `events.mjs -v` on a failing site to find the changed
   step, then update `sim.py` and `src/order.rs` together until it reports
   0 mismatches.
2. Regenerate the order fixtures: `cargo xtask fixtures-refresh order`, then
   `cargo xtask fixtures-refresh order-data`. Then run `cargo test --workspace`.
