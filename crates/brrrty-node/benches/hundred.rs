//! `cargo bench -p brrrty-node`: the spec's 100-file set, 25 each of an
//! object literal, a sync function, an async function, and `require("path")`
//! with a local `Date`. Prints the wall time from starting Node to the last
//! value; the goal on an M4 is 100 ms or less.

use std::time::Instant;

use brrrty_js::runtime::{JsResult, JsRuntime};
use brrrty_node::{RunMode, Worker, find_node};

fn main() {
    let node = match find_node(std::env::var_os("BRRRTY_NODE").as_deref()) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let dir = tempfile::tempdir().expect("temp folder");
    let mut files = Vec::new();
    for i in 0..100 {
        let body = match i % 4 {
            0 => format!(
                "module.exports = {{ id: {i}, name: \"item {i}\", tags: [\"a\", \"b\"], nested: {{ n: {i} }} }};"
            ),
            1 => format!(
                "module.exports = function () {{ return {{ id: {i}, list: Array.from({{ length: 20 }}, (_, k) => k * {i}) }}; }};"
            ),
            2 => format!(
                "module.exports = async function () {{ await null; return {{ id: {i}, ok: true }}; }};"
            ),
            _ => format!(
                "const path = require(\"path\"); module.exports = {{ id: {i}, base: path.basename(\"/a/b/c{i}.txt\"), when: new Date(2026, 0, {i} % 28 + 1).toString() }};"
            ),
        };
        let f = dir.path().join(format!("d{i}.js"));
        std::fs::write(&f, body).expect("write");
        files.push(f);
    }
    let mut times = Vec::new();
    for _ in 0..10 {
        let start = Instant::now();
        let mut w = Worker::start(&node, dir.path(), RunMode::Build).expect("start");
        for f in &files {
            match w.load(f, true) {
                JsResult::Ok(_) => {}
                other => panic!("{}: {other:?}", f.display()),
            }
        }
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        drop(w);
    }
    times.sort_by(f64::total_cmp);
    println!(
        "100 files, Node {}: median {:.1} ms, min {:.1} ms, max {:.1} ms",
        node.version_text(),
        times[times.len() / 2],
        times[0],
        times[times.len() - 1]
    );
}
