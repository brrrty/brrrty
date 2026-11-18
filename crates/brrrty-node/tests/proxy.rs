//! Brrrty's port of 11ty's computed data proxy against 11ty 3.1.8's own
//! class: 200 generated functions over generated page data
//! (`tests/proxy/gen.mjs`), each giving the same keys, in the same order,
//! as `tests/proxy/expected.json` records from 11ty (`probe-11ty.mjs`).

use std::fs;
use std::path::Path;

use brrrty_js::Value;
use brrrty_js::runtime::{JsResult, JsRuntime};
use brrrty_node::{RunMode, Worker, find_node};

#[test]
fn the_proxy_port_finds_what_11ty_finds() {
    let node = match find_node(None) {
        Ok(n) => n,
        Err(e) => {
            assert!(std::env::var_os("CI").is_none(), "CI needs Node: {e}");
            eprintln!("skipped: {e}");
            return;
        }
    };
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/proxy");
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(dir.join("cases.json")).unwrap()).unwrap();
    let expected: Vec<serde_json::Value> =
        serde_json::from_str(&fs::read_to_string(dir.join("expected.json")).unwrap()).unwrap();
    assert_eq!(cases.len(), 200);
    assert_eq!(expected.len(), cases.len());

    let t = tempfile::tempdir().unwrap();
    let fns: Vec<&str> = cases.iter().map(|c| c["fn"].as_str().unwrap()).collect();
    fs::write(
        t.path().join("fns.js"),
        format!("module.exports = {{ fns: [\n{}\n] }};\n", fns.join(",\n")),
    )
    .unwrap();
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let loaded = match w.load(&t.path().join("fns.js"), false) {
        JsResult::Ok(d) => d,
        other => panic!("{other:?}"),
    };
    let mut mismatches = Vec::new();
    for (i, (case, want)) in cases.iter().zip(&expected).enumerate() {
        let (_, handle) = &loaded.functions[i];
        let keys: Vec<String> = case["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap().to_string())
            .collect();
        let data = Value::from_json(&case["data"]);
        let page = match w.page_open(&data, &[], &keys) {
            JsResult::Ok(p) => p,
            other => panic!("{other:?}"),
        };
        let got = match w.page_vars(page, *handle, &keys) {
            JsResult::Ok(vars) => serde_json::json!({ "vars": vars }),
            JsResult::Thrown(e) => serde_json::json!({ "throws": e.name }),
            other => panic!("case {i}: {other:?}"),
        };
        w.page_close(page);
        if &got != want {
            mismatches.push(format!("case {i} `{}`: got {got}, 11ty {want}", fns[i]));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}
