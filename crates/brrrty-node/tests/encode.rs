//! The value encoding, run through real Node: every value either crosses
//! exactly or is refused with what it is.

use std::process::Command;

use brrrty_js::Value;
use brrrty_js::runtime::{Decoded, FnHandle, PathPart};
use brrrty_node::{ENCODE_SOURCE, decode};

/// `expr` encoded by the worker's code in Node, then decoded. `None` (with
/// a printed reason) when this machine has no Node; CI has one.
fn through_node(expr: &str) -> Option<Decoded> {
    let source = format!(
        "{ENCODE_SOURCE}\nconst handles = [];\nconst v = ({expr});\nprocess.stdout.write(JSON.stringify(encodeValue(v, (f) => handles.push(f) - 1)));\n"
    );
    let out = match Command::new("node")
        .args(["--input-type=module", "--eval", &source])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("CI needs Node for these tests: {e}");
            }
            eprintln!("skipped: no node on PATH ({e})");
            return None;
        }
    };
    assert!(
        out.status.success(),
        "node failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("JSON");
    Some(decode(&json).expect("decodes"))
}

fn keys(v: &Value) -> Vec<String> {
    match v {
        Value::Object(o) => o.keys().cloned().collect(),
        other => panic!("not an object: {other:?}"),
    }
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    match v {
        Value::Object(o) => &o[k],
        other => panic!("not an object: {other:?}"),
    }
}

fn key(k: &str) -> PathPart {
    PathPart::Key(k.into())
}

#[test]
fn keys_keep_javascript_order() {
    let Some(d) = through_node(r#"{"b":1,"a":2,"2":3,"1":4,"-1":5}"#) else {
        return;
    };
    assert_eq!(keys(&d.value), ["1", "2", "b", "a", "-1"]);
    assert!(d.is_exact());
}

#[test]
fn negative_zero_and_other_numbers_cross_exactly() {
    let Some(d) = through_node(
        "{z: -0, n: NaN, i: Infinity, m: -Infinity, f: 0.1 + 0.2, big: 1e21, tiny: 5e-324}",
    ) else {
        return;
    };
    let Value::Number(z) = get(&d.value, "z") else {
        panic!()
    };
    assert!(*z == 0.0 && z.is_sign_negative());
    let Value::Number(n) = get(&d.value, "n") else {
        panic!()
    };
    assert!(n.is_nan());
    assert_eq!(get(&d.value, "i"), &Value::Number(f64::INFINITY));
    assert_eq!(get(&d.value, "m"), &Value::Number(f64::NEG_INFINITY));
    assert_eq!(get(&d.value, "f"), &Value::Number(0.1 + 0.2));
    assert_eq!(get(&d.value, "big"), &Value::Number(1e21));
    assert_eq!(get(&d.value, "tiny"), &Value::Number(5e-324));
}

#[test]
fn dates_cross_to_the_millisecond() {
    let Some(d) = through_node("{d: new Date(Date.UTC(2026, 1, 3, 4, 5, 6, 7))}") else {
        return;
    };
    let Value::Date(date) = get(&d.value, "d") else {
        panic!()
    };
    assert_eq!(
        date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        "2026-02-03T04:05:06.007Z"
    );
}

#[test]
fn undefined_keeps_its_key_and_holes_read_as_undefined() {
    let Some(d) = through_node("{u: undefined, a: [1, , 3], n: null}") else {
        return;
    };
    assert_eq!(keys(&d.value), ["u", "a", "n"]);
    assert_eq!(get(&d.value, "u"), &Value::Undefined);
    // Probed with 11ty 3.1.8 (fixture js-data-values): Liquid and Nunjucks
    // `for`, `length`/`size`, `dump`/`json`, `join` and `compact` print a
    // hole exactly as they print `undefined`, so a hole crosses as one.
    let Value::Array(a) = get(&d.value, "a") else {
        panic!()
    };
    assert_eq!(
        a.as_slice(),
        [Value::Number(1.0), Value::Undefined, Value::Number(3.0)]
    );
    assert_eq!(get(&d.value, "n"), &Value::Null);
}

#[test]
fn strings_cross_whole() {
    let Some(d) =
        through_node(r#"{s: "quote \" slash \\ tab \t snow ☃ pair 😀", k: {"$o": 1, "$u": 2}}"#)
    else {
        return;
    };
    assert_eq!(
        get(&d.value, "s"),
        &Value::String("quote \" slash \\ tab \t snow \u{2603} pair \u{1f600}".into())
    );
    // A user key that looks like the wire's own stays a key.
    assert_eq!(keys(get(&d.value, "k")), ["$o", "$u"]);
}

#[test]
fn values_that_cannot_cross_are_refused_with_their_path() {
    let Some(d) = through_node(
        r#"{
            bad: new Date("x"),
            m: new Map(),
            foo: new (class Foo {})(),
            big: 10n,
            sym: Symbol("s"),
            list: [1, new Set()],
            re: /x/,
            buf: new Uint8Array(2),
            lone: "a\ud800b",
            ok: 1,
        }"#,
    ) else {
        return;
    };
    let refused: Vec<(Vec<PathPart>, &str)> = d
        .refused
        .iter()
        .map(|(p, w)| (p.clone(), w.as_str()))
        .collect();
    assert_eq!(
        refused,
        vec![
            (vec![key("bad")], "an invalid Date"),
            (vec![key("m")], "a Map object"),
            (vec![key("foo")], "a Foo object"),
            (vec![key("big")], "a BigInt"),
            (vec![key("sym")], "a Symbol"),
            (vec![key("list"), PathPart::Index(1)], "a Set object"),
            (vec![key("re")], "a RegExp object"),
            (vec![key("buf")], "a Uint8Array object"),
            (vec![key("lone")], "a string with a lone surrogate"),
        ]
    );
    // Refused values never print: they are undefined until refused.
    assert_eq!(get(&d.value, "m"), &Value::Undefined);
    assert_eq!(get(&d.value, "ok"), &Value::Number(1.0));
}

#[test]
fn a_cycle_is_refused_naming_where_it_points() {
    let Some(d) =
        through_node("(() => { const a = {x: {}}; a.x.back = a.x; a.x.top = a; return {a}; })()")
    else {
        return;
    };
    assert_eq!(
        d.refused,
        vec![
            (
                vec![key("a"), key("x"), key("back")],
                "a circular reference to data.a.x".to_string()
            ),
            (
                vec![key("a"), key("x"), key("top")],
                "a circular reference to data.a".to_string()
            ),
        ]
    );
}

#[test]
fn a_shared_object_that_is_not_a_cycle_crosses() {
    let Some(d) = through_node("(() => { const s = {v: 1}; return {a: s, b: [s, s]}; })()") else {
        return;
    };
    assert!(d.is_exact(), "{:?}", d.refused);
}

#[test]
fn functions_are_handles() {
    let Some(d) = through_node("{f() {}, nested: {g: () => 1}, c: class {}}") else {
        return;
    };
    assert_eq!(
        d.functions,
        vec![
            (vec![key("f")], FnHandle(0)),
            (vec![key("nested"), key("g")], FnHandle(1)),
            (vec![key("c")], FnHandle(2)),
        ]
    );
    assert_eq!(get(&d.value, "f"), &Value::Undefined);
}

#[test]
fn a_function_at_the_root_is_a_handle_at_the_root() {
    let Some(d) = through_node("function () {}") else {
        return;
    };
    assert_eq!(d.functions, vec![(vec![], FnHandle(0))]);
}

#[test]
fn symbol_keys_and_hidden_keys_are_dropped() {
    let Some(d) = through_node(
        r#"(() => { const o = {[Symbol("s")]: 1, shown: 2}; Object.defineProperty(o, "hidden", {value: 3, enumerable: false}); return o; })()"#,
    ) else {
        return;
    };
    assert_eq!(keys(&d.value), ["shown"]);
}

#[test]
fn a_steady_getter_is_read_and_a_changing_one_refused() {
    let Some(d) = through_node(
        r#"(() => { let n = 0; return {get steady() { return "s"; }, get counter() { n += 1; return n; }}; })()"#,
    ) else {
        return;
    };
    assert_eq!(get(&d.value, "steady"), &Value::String("s".into()));
    assert_eq!(
        d.refused,
        vec![(
            vec![key("counter")],
            "a getter that returns a different value each time it is read".to_string()
        )]
    );
}

#[test]
fn null_prototype_objects_are_plain() {
    let Some(d) = through_node(r#"Object.assign(Object.create(null), {a: 1})"#) else {
        return;
    };
    assert_eq!(keys(&d.value), ["a"]);
    assert!(d.is_exact());
}

#[test]
fn malformed_wire_forms_are_errors() {
    for bad in [
        r#"{"$o": 1}"#,
        r#"{"$o": [["a"]]}"#,
        r#"{"$n": "1"}"#,
        r#"{"$d": "x"}"#,
        r#"{"$f": -1}"#,
        r#"{"$x": 1}"#,
        r#"{"plain": 1}"#,
        r#"{"$hole": 1}"#,
    ] {
        let json: serde_json::Value = serde_json::from_str(bad).unwrap();
        assert!(decode(&json).is_err(), "{bad} should not decode");
    }
}
