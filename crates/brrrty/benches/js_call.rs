//! `cargo bench -p brrrty --bench js_call`: what one call of a JavaScript
//! function from a template costs, through the real render path.
//!
//! The site has 2,000 Nunjucks pages; each calls a trivial function from a
//! JavaScript data file 20 times. The same site is built with the function
//! (Node on) and with a plain Nunjucks expression of the same shape in its
//! place (no Node), on 1 render thread and on all of them. The extra time
//! over the plain build, divided by 40,000 calls, is the cost per call.
//! Set `BRRRTY_BENCH_PAGES` and `BRRRTY_BENCH_CALLS` to change the size.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Instant;

fn env_n(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn write(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn site(pages: usize, calls: usize, js: bool) -> tempfile::TempDir {
    let t = tempfile::tempdir().expect("temp folder");
    let d = t.path();
    if js {
        write(d, "brrrty.config.rhai", "config.set_js_data(\"node\");\n");
        write(
            d,
            "src/_data/fx.js",
            "module.exports = { inc: (n) => n + 1 };\n",
        );
    }
    let call = if js {
        "fx.inc(page.fileSlug | length)"
    } else {
        "(page.fileSlug | length) + 1"
    };
    let body = (0..calls)
        .map(|i| format!("{{{{ {call} + {i} }}}}\n"))
        .collect::<String>();
    for p in 0..pages {
        write(d, &format!("src/p{p}.njk"), &body);
    }
    t
}

fn build(dir: &Path, threads: Option<usize>) -> f64 {
    let mut c = Command::new(env!("CARGO_BIN_EXE_brrrty"));
    c.current_dir(dir)
        .args(["build", "--input", "src"])
        .env_remove("SOURCE_DATE_EPOCH");
    if let Some(n) = threads {
        c.env("RAYON_NUM_THREADS", n.to_string());
    }
    let start = Instant::now();
    let o = c.output().expect("run brrrty");
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    assert!(
        o.status.success(),
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    ms
}

fn median(dir: &Path, threads: Option<usize>) -> f64 {
    let mut times: Vec<f64> = (0..5)
        .map(|_| {
            let _ = fs::remove_dir_all(dir.join("_site"));
            build(dir, threads)
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn main() {
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipped: no node on PATH");
        return;
    }
    let pages = env_n("BRRRTY_BENCH_PAGES", 2000);
    let calls = env_n("BRRRTY_BENCH_CALLS", 20);
    let with = site(pages, calls, true);
    let plain = site(pages, calls, false);
    let all = std::thread::available_parallelism().map_or(1, |n| n.get());
    let total = (pages * calls) as f64;
    println!("{pages} pages x {calls} calls = {total} calls, {all} cores");
    for (label, threads) in [("1 thread", Some(1)), ("all threads", None)] {
        let base = median(plain.path(), threads);
        let js = median(with.path(), threads);
        println!(
            "{label}: plain {base:.0} ms, with JS calls {js:.0} ms, extra {:.0} ms = {:.1} us per call",
            js - base,
            (js - base) * 1000.0 / total
        );
    }
}
