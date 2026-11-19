//! The data files' values, frozen instead of walked: when the worker
//! freezes, what it then refuses to vouch for, and that the walk decides
//! the same site as it always did.

use std::fs;

use brrrty_js::Value;
use brrrty_js::runtime::{JsResult, JsRuntime, Settle, This};
use brrrty_node::{RunMode, Worker, find_node};

fn node() -> Option<brrrty_node::NodeInfo> {
    match find_node(None) {
        Ok(n) => {
            // Before 22.15 Node can't show the worker every file that
            // loads, so it always walks.
            if !n.load_hooks {
                eprintln!("skipped: Node {} has no load hooks", n.version_text());
                return None;
            }
            Some(n)
        }
        Err(e) => {
            if std::env::var_os("CI").is_some() {
                panic!("CI needs Node for these tests: {e}");
            }
            eprintln!("skipped: {e}");
            None
        }
    }
}

fn site(files: &[(&str, &str)]) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for (rel, body) in files {
        let p = t.path().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }
    t
}

/// An ES module project with one data file, `d.js`.
fn module_site(data: &str) -> tempfile::TempDir {
    site(&[("package.json", r#"{"type":"module"}"#), ("d.js", data)])
}

fn ok<T: std::fmt::Debug>(r: JsResult<T>) -> T {
    match r {
        JsResult::Ok(v) => v,
        other => panic!("{other:?}"),
    }
}

fn handle(v: &Value, k: &str) -> brrrty_js::FnHandle {
    match v {
        Value::Object(o) => match &o[k] {
            Value::Function(h) => *h,
            other => panic!("not a function: {other:?}"),
        },
        other => panic!("not an object: {other:?}"),
    }
}

/// Start a worker on the site, load `d.js` and begin the calls.
fn begun(node: &brrrty_node::NodeInfo, t: &tempfile::TempDir) -> (Worker, Value) {
    let mut w = Worker::start(node, t.path(), RunMode::Build).expect("starts");
    let d = ok(w.load(&t.path().join("d.js"), true)).value;
    ok(w.begin_calls());
    (w, d)
}

fn call(w: &mut Worker, d: &Value, name: &str) -> JsResult<brrrty_js::runtime::Decoded> {
    w.call(handle(d, name), This::Value(d), &[], Settle::Await)
}

/// How a build that froze ends when it can't vouch for what it saw: some
/// request, at the latest the final check, asks for the build again.
fn asks_again(w: &mut Worker, d: &Value, names: &[&str]) -> bool {
    for name in names {
        if let JsResult::Failed(f) = call(w, d, name) {
            return f.again;
        }
    }
    matches!(w.verify(), JsResult::Failed(f) if f.again)
}

/// What the walk says of the same calls: `Ok` or the refusal.
fn walked(
    node: &brrrty_node::NodeInfo,
    t: &tempfile::TempDir,
    names: &[&str],
) -> Result<(), String> {
    let (mut w, d) = begun(node, t);
    assert!(w.exports_frozen().is_err(), "the folder's next build walks");
    for name in names {
        match call(&mut w, &d, name) {
            JsResult::Ok(_) | JsResult::Thrown(_) => {}
            JsResult::Refused(r) => return Err(r.why),
            JsResult::Failed(f) => panic!("{f:?}"),
        }
    }
    match w.verify() {
        JsResult::Ok(()) => Ok(()),
        JsResult::Refused(r) => Err(r.why),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_strict_site_is_frozen() {
    let Some(node) = node() else { return };
    let t = module_site("export default { list: [1, 2], f: () => 1 };");
    let (w, _) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    drop(w);

    let t = site(&[(
        "d.js",
        "// A comment first.\n'use strict';\nmodule.exports = { list: [1, 2], f: () => 1 };",
    )]);
    let (w, _) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
}

/// CommonJS with no "use strict", in which strict mode changes nothing
/// without a sound: loaded as strict, and frozen.
#[test]
fn sloppy_commonjs_that_strict_mode_leaves_alone_is_frozen() {
    let Some(node) = node() else { return };
    for body in [
        "module.exports = { list: [1, 2], f: () => 1 };",
        "#!/usr/bin/env node\nmodule.exports = { list: [1, 2], f: () => 1 };",
        // `this` read for its properties, in a method and in a constructor.
        "function Box(n) { this.n = n; }\nBox.prototype.get = function () { return this.n; };\nmodule.exports = { list: [new Box(1).get()], label: 'x', f() { return this.label; } };",
        // `this` at the top of a module is its exports either way.
        "var self = this;\nmodule.exports = { list: [typeof self], f: function () { return 1; } };",
        // A function that is strict already keeps its own rules.
        "module.exports = { list: [], f: function () { 'use strict'; return this; } };",
        // A file it requires, sloppy too.
        "module.exports = { list: require('./lib.js'), f: () => 1 };",
    ] {
        let t = site(&[("d.js", body), ("lib.js", "module.exports = [1, 2];")]);
        let (w, _) = begun(&node, &t);
        assert_eq!(w.exports_frozen(), Ok(()), "{body}");
    }
}

/// CommonJS that strict mode would change in silence, or that it refuses,
/// stays as written, and its data is walked.
#[test]
fn sloppy_commonjs_that_strict_mode_would_change_is_walked() {
    let Some(node) = node() else { return };
    for body in [
        // The arguments object stops tracking the parameters.
        "module.exports = { list: [], f: function (a) { a = 2; return arguments[0]; } };",
        // `eval`'s variables stop leaking.
        "module.exports = { list: [], f: function () { eval('var x = 1'); return typeof x; } };",
        // `this` as a value: the global object, or undefined.
        "module.exports = { list: [], f: function () { return this; } };",
        "module.exports = { list: [], f: function () { return typeof this; } };",
        "module.exports = { list: [], f: function () { return this?.x; } };",
        // A function declared in a block leaks out of it.
        "if (true) { function inner() { return 1; } }\nmodule.exports = { list: [], f: function () { return inner(); } };",
        // What strict mode refuses outright.
        "var n = 010;\nmodule.exports = { list: [n], f: () => 1 };",
        "var o = { a: 1 };\nwith (o) { var b = a; }\nmodule.exports = { list: [b], f: () => 1 };",
        "var x = 1;\nmodule.exports = { list: [delete x], f: () => 1 };",
        "module.exports = { list: [], f: function (a, a) { return a; } };",
    ] {
        let t = site(&[("d.js", body)]);
        let (w, _) = begun(&node, &t);
        let why = w.exports_frozen().unwrap_err();
        assert!(why.contains("d.js is not in strict mode"), "{body}: {why}");
    }
    // A file it requires.
    let t = site(&[
        (
            "d.js",
            "'use strict';\nmodule.exports = { list: require('./lib.js'), f: () => 1 };",
        ),
        ("lib.js", "module.exports = [arguments.length];"),
    ]);
    let (w, _) = begun(&node, &t);
    let why = w.exports_frozen().unwrap_err();
    assert!(why.contains("lib.js is not in strict mode"), "{why}");
}

/// Where strict mode throws and sloppy mode goes on, a file loaded as
/// strict sends the build back: the next one loads every file as written.
#[test]
fn what_strict_mode_makes_loud_sends_the_build_back() {
    let Some(node) = node() else { return };
    // (the file, what `f` gives as written)
    let cases: &[(&str, Value)] = &[
        // An assignment to a name nothing declares makes a global.
        (
            "module.exports = { f: function () { leaked = 5; return leaked; } };",
            Value::Number(5.0),
        ),
        // A write a frozen object drops.
        (
            "var own = Object.freeze({ n: 1 });\nmodule.exports = { f: function () { own.n = 2; return own.n; } };",
            Value::Number(1.0),
        ),
        // The same, with the error caught.
        (
            "var own = Object.freeze({ n: 1 });\nmodule.exports = { f: function () { try { own.n = 2; } catch (e) { return 'threw'; } return own.n; } };",
            Value::Number(1.0),
        ),
        // `this` is the global object in a plain call.
        (
            "function plain() { return this.Array === Array; }\nmodule.exports = { f: function () { return plain(); } };",
            Value::Bool(true),
        ),
        // A property set on a string goes nowhere.
        (
            "module.exports = { f: function () { var s = 'abc'; s.extra = 1; return s.length; } };",
            Value::Number(3.0),
        ),
    ];
    for (body, want) in cases {
        let t = site(&[("d.js", body)]);
        let (mut w, d) = begun(&node, &t);
        assert_eq!(w.exports_frozen(), Ok(()), "{body}");
        assert!(asks_again(&mut w, &d, &["f"]), "{body}");
        drop(w);
        let (mut w, d) = begun(&node, &t);
        // The walk says why: the build before it was sent back, not
        // anything the user set.
        let why = w.exports_frozen().unwrap_err();
        assert!(
            why.starts_with("this folder's last build was sent back: "),
            "{body}: {why}"
        );
        assert_eq!(&ok(call(&mut w, &d, "f")).value, want, "{body}");
        assert_eq!(w.verify(), JsResult::Ok(()), "{body}");
    }
    // While the file loads, too.
    let t = site(&[(
        "d.js",
        "leaked = 7;\nmodule.exports = { n: leaked, f: () => 1 };",
    )]);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    match w.load(&t.path().join("d.js"), true) {
        JsResult::Failed(f) => assert!(f.again, "{f:?}"),
        other => panic!("{other:?}"),
    }
    drop(w);
    let mut w = Worker::start(&node, t.path(), RunMode::Build).expect("starts");
    let d = ok(w.load(&t.path().join("d.js"), true)).value;
    match &d {
        Value::Object(o) => assert_eq!(o["n"], Value::Number(7.0)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_frozen_build_that_changes_nothing_stays_frozen_and_passes() {
    let Some(node) = node() else { return };
    let t = module_site(
        r#"
const list = [{ n: 1 }, { n: 2 }];
const when = new Date(0);
export default {
  list,
  when,
  // Reads, copies, and errors that have nothing to do with the data.
  read: () => list.map((x) => x.n).join(),
  copy: () => [...list].sort((a, b) => b.n - a.n).map((x) => ({ ...x, m: 1 })).length,
  time: () => when.getTime() + new Date(when).setFullYear(2000) * 0,
  caught: () => { try { null.x; } catch { return "caught"; } },
  thrown: () => { try { throw new TypeError("mine"); } catch (e) { return e.message; } },
  global: () => typeof Function("return this")(),
  keys: () => Object.keys(list[0]).join() + JSON.stringify(list),
};
"#,
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    for (name, want) in [
        ("read", Value::String("1,2".into())),
        ("copy", Value::Number(2.0)),
        ("time", Value::Number(0.0)),
        ("caught", Value::String("caught".into())),
        ("thrown", Value::String("mine".into())),
        ("global", Value::String("object".into())),
        ("keys", Value::String(r#"n[{"n":1},{"n":2}]"#.into())),
    ] {
        assert_eq!(ok(call(&mut w, &d, name)).value, want, "{name}");
    }
    assert_eq!(w.verify(), JsResult::Ok(()));
}

/// Each way a call can try to change, or look at the frozenness of, a
/// value a data file gave: the frozen build asks for the build again,
/// and the walk then refuses what it always refused and passes the rest.
#[test]
fn every_kind_of_write_sends_the_build_back_to_the_walk() {
    let Some(node) = node() else { return };
    // (the functions to call, in order; whether the walk refuses)
    let cases: &[(&str, &[&str], bool)] = &[
        // Writes that stay.
        ("set: () => { item.flag = true; }", &["set"], true),
        ("add: () => { item.extra = 1; }", &["add"], true),
        ("del: () => { delete item.n; }", &["del"], true),
        ("idx: () => { list[0] = 9; }", &["idx"], true),
        ("len: () => { list.length = 0; }", &["len"], true),
        ("push: () => { list.push(3); }", &["push"], true),
        (
            "sort: () => { list.sort((a, b) => b.n - a.n); }",
            &["sort"],
            true,
        ),
        (
            "assign: () => { Object.assign(item, { n: 5 }); }",
            &["assign"],
            true,
        ),
        ("date: () => { when.setFullYear(1999); }", &["date"], true),
        (
            "refl: () => { Reflect.set(item, 'n', 7); }",
            &["refl"],
            true,
        ),
        (
            "def: () => { Object.defineProperty(item, 'n', { value: 8 }); }",
            &["def"],
            true,
        ),
        (
            "proto: () => { Object.setPrototypeOf(item, null); }",
            &["proto"],
            true,
        ),
        // The same, with the error a frozen object throws swallowed.
        (
            "quiet: () => { try { item.flag = true; } catch {} return 1; }",
            &["quiet"],
            true,
        ),
        (
            "quietPush: () => { try { list.push(3); } catch {} return 1; }",
            &["quietPush"],
            true,
        ),
        // Writes undone by a later call: the data ends as it began, and
        // the walk after the first call still sees them.
        (
            "on: () => { try { item.flag = true; } catch {} }, off: () => { try { delete item.flag; } catch {} }",
            &["on", "off"],
            true,
        ),
        (
            "in_: () => { try { list.push(3); } catch {} }, out: () => { try { list.pop(); } catch {} }",
            &["in_", "out"],
            true,
        ),
        // Writes that change nothing, and questions: the walk passes them.
        ("same: () => { item.n = item.n; }", &["same"], false),
        (
            "there: () => { list.sort((a, b) => a.n - b.n); }",
            &["there"],
            false,
        ),
        ("asks: () => Object.isFrozen(item)", &["asks"], false),
        (
            "attrs: () => Object.getOwnPropertyDescriptor(item, 'n').writable",
            &["attrs"],
            false,
        ),
        // Code the worker can't vouch for.
        ("ev: () => (0, eval)('1 + 1')", &["ev"], false),
        ("fn: () => new Function('a', 'return a')(1)", &["fn"], false),
        (
            "late: async () => (await import('./sloppy.cjs')).default.n",
            &["late"],
            false,
        ),
        (
            "vm: async () => typeof (await import('node:vm')).Script",
            &["vm"],
            false,
        ),
    ];
    // As an ES module, and as CommonJS with no "use strict" (loaded as
    // strict: nothing in these files is changed by it in silence).
    for module in [true, false] {
        for (body, names, refuses) in cases {
            // A CommonJS file that says `eval` stays as written, and is
            // walked from the start.
            if !module && (body.contains("import(") || body.contains("eval")) {
                continue;
            }
            let (package, open) = if module {
                (r#"{"type":"module"}"#, "export default")
            } else {
                ("{}", "module.exports =")
            };
            let t = site(&[
                ("package.json", package),
                (
                    "sloppy.cjs",
                    "module.exports = { n: (function () { return arguments.length; })() };",
                ),
                (
                    "d.js",
                    &format!(
                        "const item = {{ n: 1 }};\nconst list = [item, {{ n: 2 }}];\nconst when = new Date(0);\n{open} {{ list, when, {body} }};\n"
                    ),
                ),
            ]);
            let (mut w, d) = begun(&node, &t);
            assert_eq!(w.exports_frozen(), Ok(()), "{body} (module: {module})");
            assert!(
                asks_again(&mut w, &d, names),
                "{body} (module: {module}): the frozen build didn't ask again"
            );
            drop(w);
            let again = walked(&node, &t, names);
            assert_eq!(
                again.is_err(),
                *refuses,
                "{body} (module: {module}): the walk gave {again:?}"
            );
            if let Err(why) = again {
                assert!(why.contains("changes the data"), "{body}: {why}");
            }
        }
    }
}

#[test]
fn what_freezing_cant_hold_still_is_walked_as_before() {
    let Some(node) = node() else { return };
    // A getter's value follows state no property holds.
    let t = module_site(
        "let n = 0;\nexport default { box: { get count() { return n; } }, bump: () => { n += 1; return n; } };",
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    ok(call(&mut w, &d, "bump"));
    match w.verify() {
        JsResult::Refused(r) => {
            assert!(r.impure, "{r:?}");
            assert!(r.why.contains("changes the data d.js gives"), "{}", r.why);
        }
        other => panic!("{other:?}"),
    }

    // A built-in prototype isn't frozen.
    let t = module_site(
        "export default { list: [1], patch: () => { Array.prototype.last = function () { return this[this.length - 1]; }; return 1; } };",
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    ok(call(&mut w, &d, "patch"));
    match w.verify() {
        JsResult::Refused(r) => assert!(r.why.contains("built-in prototypes"), "{}", r.why),
        other => panic!("{other:?}"),
    }

    // A RegExp's `lastIndex` moves as it is used, and comes back.
    let t = module_site(
        "const re = /a/g;\nexport default { re: { re }, count: () => { let n = 0; while (re.exec('aaa')) n += 1; return n; } };",
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    assert_eq!(ok(call(&mut w, &d, "count")).value, Value::Number(3.0));
    assert_eq!(w.verify(), JsResult::Ok(()));
}

#[test]
fn naming_a_function_leaves_the_watcher_watching() {
    let Some(node) = node() else { return };
    // A refusal names its function through the inspector, which the
    // watcher also uses.
    let t = module_site(
        "const item = { n: 1 };\nexport default { item, poke: (o) => { o.x = 1; }, quiet: () => { try { item.n = 2; } catch {} return 1; } };",
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    let arg = Value::from_json(&serde_json::json!({}));
    match w.call(handle(&d, "poke"), This::Value(&d), &[arg], Settle::Await) {
        JsResult::Refused(r) => assert!(r.why.contains("d.js:2:"), "{}", r.why),
        other => panic!("{other:?}"),
    }
    assert!(asks_again(&mut w, &d, &["quiet"]));
}

/// An error both modes throw, in a file loaded as strict, is the site's
/// own business: the build stays frozen.
#[test]
fn errors_strict_mode_shares_with_sloppy_mode_change_nothing() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "d.js",
        r#"var box = { n: 1 };
module.exports = {
  list: [box],
  read: function () { try { return null.x; } catch (e) { return e.name; } },
  name: function () { try { return missing; } catch (e) { return e.name; } },
  call: function () { try { return box.nope(); } catch (e) { return e.name; } },
  plus: function () { try { missing += 1; } catch (e) { return e.name; } },
  mine: function () { try { throw new TypeError("mine"); } catch (e) { return e.message; } },
  own: function () { return this.list.length; }
};
"#,
    )]);
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    for (name, want) in [
        ("read", Value::String("TypeError".into())),
        ("name", Value::String("ReferenceError".into())),
        ("call", Value::String("TypeError".into())),
        ("plus", Value::String("ReferenceError".into())),
        ("mine", Value::String("mine".into())),
        ("own", Value::Number(1.0)),
    ] {
        assert_eq!(ok(call(&mut w, &d, name)).value, want, "{name}");
    }
    assert_eq!(w.verify(), JsResult::Ok(()));
}

/// Ways around the checks that keep sloppy code from running on frozen
/// values, each closed: the file writes to its list in sloppy code the
/// worker used to take for strict (or never saw), where the write does
/// nothing and nothing throws. The build must not stay frozen, and the
/// walk then refuses the write as it always did.
#[test]
fn sloppy_code_in_disguise_never_runs_on_frozen_values() {
    let Some(node) = node() else { return };
    let cases: &[(&str, &str)] = &[
        // A builtin that compiles code, reached without `require`.
        (
            "getBuiltinModule",
            "'use strict';\nconst vm = process.getBuiltinModule('node:vm');\nconst list = ['a'];\nconst add = vm.runInThisContext(\"(function (l) { l[l.length] = 'b'; })\");\nmodule.exports = { list, add() { add(list); return ''; } };",
        ),
        (
            "getBuiltinModule, no prefix",
            "'use strict';\nconst vm = process.getBuiltinModule('vm');\nconst list = ['a'];\nconst add = new vm.Script(\"(function (l) { l[l.length] = 'b'; })\").runInThisContext();\nmodule.exports = { list, add() { add(list); return ''; } };",
        ),
        // A string that only looks like a directive: the next line goes
        // on with the same expression.
        (
            "comma after the string",
            "\"use strict\"\n,0;\nconst list = ['a'];\nmodule.exports = { list, add() { list[list.length] = 'b'; return ''; } };",
        ),
        (
            "member of the string",
            "'use strict'\n.length;\nconst list = ['a'];\nmodule.exports = { list, add() { list[list.length] = 'b'; return ''; } };",
        ),
        (
            "an empty statement first",
            ";'use strict';\nconst list = ['a'];\nmodule.exports = { list, add() { list[list.length] = 'b'; return ''; } };",
        ),
        // The same in a function made from text.
        (
            "new Function body",
            "'use strict';\nconst list = ['a'];\nconst add = new Function('l', '\"use strict\"\\n,0;\\nl[l.length] = \"b\";');\nmodule.exports = { list, add() { add(list); return ''; } };",
        ),
        // Parameters are code too.
        (
            "Function parameters",
            "'use strict';\nconst list = ['a'];\nconst add = Function(\"l, _ = (l[l.length] = 'b')\", 'return this');\nmodule.exports = { list, add() { add(list); return ''; } };",
        ),
        // The error's name, changed for the length of one call.
        (
            "TypeError.prototype.name",
            "'use strict';\nconst list = ['a'];\nmodule.exports = { list, add() { const n = TypeError.prototype.name; TypeError.prototype.name = '\\n'; try { list.push('b'); } catch (e) {} TypeError.prototype.name = n; return ''; } };",
        ),
        (
            "Error.prototype.toString",
            "'use strict';\nconst list = ['a'];\nmodule.exports = { list, add() { const s = Error.prototype.toString; Error.prototype.toString = () => 'fine'; try { list.push('b'); } catch (e) {} Error.prototype.toString = s; return ''; } };",
        ),
    ];
    for (what, body) in cases {
        let t = site(&[("d.js", body)]);
        let (mut w, d) = begun(&node, &t);
        let frozen = w.exports_frozen().is_ok();
        if frozen {
            assert!(asks_again(&mut w, &d, &["add"]), "{what}: stayed frozen");
        }
        drop(w);
        let refused = if frozen {
            walked(&node, &t, &["add"])
        } else {
            // It walked from the start: the same worker decides.
            let (mut w, d) = begun(&node, &t);
            match call(&mut w, &d, "add") {
                JsResult::Refused(r) => Err(r.why),
                JsResult::Ok(_) => match w.verify() {
                    JsResult::Refused(r) => Err(r.why),
                    other => panic!("{what}: {other:?}"),
                },
                other => panic!("{what}: {other:?}"),
            }
        };
        let why = refused.expect_err(what);
        assert!(why.contains("changes the data"), "{what}: {why}");
    }
}

/// A function made from text is checked as the text V8 compiles: an
/// object that gives other text the second time it is asked is asked
/// once.
#[test]
fn a_function_body_is_made_text_once() {
    let Some(node) = node() else { return };
    let t = site(&[(
        "d.js",
        "'use strict';\nconst body = { n: 0, toString() { return this.n++ ? 'return 2' : 'return this'; } };\nconst f = Function(body);\nmodule.exports = { list: [1], kind: () => typeof f() };",
    )]);
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    assert_eq!(
        ok(call(&mut w, &d, "kind")).value,
        Value::String("object".into())
    );
    assert_eq!(w.verify(), JsResult::Ok(()));
}

/// A stack trace's own objects hide a frame's function when the frame is
/// strict, so a file loaded as strict shows differently to a formatter
/// of the site's own, however it is put in place: the build runs again
/// with every file as written, and then gives what 11ty gives.
#[test]
fn a_stack_formatter_of_the_sites_own_sends_a_strictified_build_back() {
    let Some(node) = node() else { return };
    let ways = [
        "Error.prepareStackTrace = function (e, cs) { return cs; };",
        "Object.defineProperty(Error, 'prepareStackTrace', { value: function (e, cs) { return cs; }, writable: true, configurable: true });",
        "delete Error.prepareStackTrace; Error.prepareStackTrace = function (e, cs) { return cs; };",
        "Reflect.defineProperty(Error, 'prepareStackTrace', { value: function (e, cs) { return cs; }, writable: true, configurable: true });",
    ];
    for way in ways {
        // Both files are sloppy as written, and loaded as strict.
        let t = site(&[
            (
                "d.js",
                "var who = require('./who.js');\nmodule.exports = { list: [1], name: function name() { return who(); } };",
            ),
            (
                "who.js",
                &format!(
                    "module.exports = function who() {{\n  var saved = Error.prepareStackTrace;\n  {way}\n  var cs = new Error().stack;\n  Error.prepareStackTrace = saved;\n  var f = cs[1].getFunction();\n  return f ? 'fn ' + f.name : 'hidden';\n}};"
                ),
            ),
        ]);
        let (mut w, d) = begun(&node, &t);
        assert_eq!(w.exports_frozen(), Ok(()), "{way}");
        assert!(asks_again(&mut w, &d, &["name"]), "{way}");
        drop(w);
        let (mut w, d) = begun(&node, &t);
        assert!(w.exports_frozen().is_err(), "{way}");
        assert_eq!(
            ok(call(&mut w, &d, "name")).value,
            Value::String("fn name".into()),
            "{way}"
        );
    }
}

/// The text of a stack trace, as the site's code reads it, has the
/// columns the file has, though a file loaded as strict has "use strict"
/// ahead of its first line; and Node's own frames are named as they are
/// without the worker.
#[test]
fn stack_text_is_as_the_file_was_written() {
    let Some(node) = node() else { return };
    let body = "var here = new Error('x').stack.split('\\n').slice(1, 3).join('|'); module.exports = { list: [1], here: here, s: function () { return new Error('e').stack.split('\\n')[1]; }, t: function () { return [1].map(function g() { return new Error('z').stack.split('\\n')[1]; })[0]; } };";
    let t = site(&[("d.js", body)]);
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    let column = |needle: &str| body.find(needle).unwrap() + 1;
    let text = |v: Value| match v {
        Value::String(s) => s.to_string(),
        other => panic!("{other:?}"),
    };
    let s = text(ok(call(&mut w, &d, "s")).value);
    assert!(
        s.ends_with(&format!("d.js:1:{})", column("new Error('e')"))),
        "{s}"
    );
    let g = text(ok(call(&mut w, &d, "t")).value);
    assert!(
        g.ends_with(&format!("d.js:1:{})", column("new Error('z')"))),
        "{g}"
    );
    let Value::Object(o) = &d else { panic!() };
    let here = text(o["here"].clone());
    let frames: Vec<&str> = here.split('|').collect();
    assert!(
        frames[0].ends_with(&format!("d.js:1:{})", column("new Error('x')"))),
        "{here}"
    );
    assert!(
        frames[1]
            .trim()
            .starts_with("at Module._compile (node:internal/modules/cjs/loader"),
        "{here}"
    );
    assert_eq!(w.verify(), JsResult::Ok(()));
}

/// The worker's stand-ins for built-in functions print as the built-ins
/// do, and a look at a property the worker defines its own way sends the
/// build back to where nothing stands in.
#[test]
fn stand_ins_look_like_what_they_stand_in_for() {
    let Some(node) = node() else { return };
    let t = module_site(
        "export default { list: [1],\n  native: () => [Object.freeze, Object.getOwnPropertyDescriptor, Reflect.defineProperty, Function, Function.prototype.toString, Error.prepareStackTrace].map((f) => /\\{\\s*\\[native code\\]\\s*\\}$/.test(Function.prototype.toString.call(f)) || /internalPrepareStackTrace/.test(String(f))).join(),\n  names: () => [Object.freeze.name, Function.name, Error.prepareStackTrace.name, Error.prepareStackTrace.length].join(),\n  evalShape: () => typeof Object.getOwnPropertyDescriptor(globalThis, 'eval').value,\n};",
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    assert_eq!(
        ok(call(&mut w, &d, "native")).value,
        Value::String("true,true,true,true,true,true".into())
    );
    assert_eq!(
        ok(call(&mut w, &d, "names")).value,
        Value::String("freeze,Function,ErrorPrepareStackTrace,2".into())
    );
    // `eval` is a property with a getter while the worker freezes: asking
    // how it is defined would show that.
    assert!(asks_again(&mut w, &d, &["evalShape"]));
    drop(w);
    let (mut w, d) = begun(&node, &t);
    assert!(w.exports_frozen().is_err());
    assert_eq!(
        ok(call(&mut w, &d, "evalShape")).value,
        Value::String("function".into())
    );
}

/// A write to an argument after the call returned (in a promise nothing
/// waited for) is seen at the next call: the build is refused instead of
/// the change reaching later pages.
#[test]
fn a_write_after_the_call_returned_is_refused() {
    let Some(node) = node() else { return };
    let t = module_site(
        "export default { list: [1], stash: (o) => { Promise.resolve().then(() => { o.extra = 1; }); return ''; }, count: (o) => Object.keys(o).length };",
    );
    let (mut w, d) = begun(&node, &t);
    let arg = Value::from_json(&serde_json::json!({ "a": 1 }));
    let held = arg.clone();
    assert_eq!(
        ok(w.call(
            handle(&d, "stash"),
            This::Value(&d),
            std::slice::from_ref(&arg),
            Settle::AsIs
        ))
        .value,
        Value::String("".into())
    );
    match w.call(
        handle(&d, "count"),
        This::Value(&d),
        std::slice::from_ref(&held),
        Settle::Await,
    ) {
        JsResult::Refused(r) => assert!(r.why.contains("after it returns"), "{}", r.why),
        other => panic!("{other:?}"),
    }
}

/// Questions about an argument (Brrrty's own wrapper around a page's
/// data, not a value a data file gave) leave the build frozen.
#[test]
fn descriptors_of_an_argument_leave_the_build_frozen() {
    let Some(node) = node() else { return };
    let t = module_site(
        "export default { list: [1], copy: (o) => Object.keys(Object.getOwnPropertyDescriptors(o)).join() + Object.isFrozen(o) + Object.isExtensible(o.in) };",
    );
    let (mut w, d) = begun(&node, &t);
    assert_eq!(w.exports_frozen(), Ok(()));
    let arg = Value::from_json(&serde_json::json!({ "a": 1, "in": { "b": 2 } }));
    assert_eq!(
        ok(w.call(
            handle(&d, "copy"),
            This::Value(&d),
            &[arg],
            Settle::Await
        ))
        .value,
        Value::String("a,infalsetrue".into())
    );
    assert_eq!(w.verify(), JsResult::Ok(()));
}

/// A call that changes a shared value makes the worker forget the values
/// it kept; the next request that refers to one is told so, and Brrrty
/// sends the values again with it. The caller sees only the answer.
#[test]
fn a_request_for_a_forgotten_value_goes_again_with_it() {
    let Some(node) = node() else { return };
    let t = module_site(
        "export default { list: [1], poke: (o) => { o.in.x = 1; return ''; }, count: (o, p) => Object.keys(o.in).length + (p ? p.list.length : 0) };",
    );
    let (mut w, d) = begun(&node, &t);
    // Held twice, so the workers keep them under an id.
    let arg = Value::from_json(&serde_json::json!({ "in": { "a": 1 } }));
    let other = Value::from_json(&serde_json::json!({ "list": [1, 2, 3] }));
    let keep = (arg.clone(), other.clone());
    let n = |w: &mut Worker, args: &[Value]| {
        ok(w.call(handle(&d, "count"), This::Value(&d), args, Settle::Await)).value
    };
    assert_eq!(n(&mut w, &[arg.clone(), other.clone()]), Value::Number(4.0));
    match w.call(
        handle(&d, "poke"),
        This::Value(&d),
        std::slice::from_ref(&arg),
        Settle::Await,
    ) {
        JsResult::Refused(r) => assert!(r.why.contains("changes the data it is given"), "{}", r.why),
        other => panic!("{other:?}"),
    }
    // Both were defined before the change, and are known by id again
    // after: as Brrrty has them, not as the call left them.
    assert_eq!(n(&mut w, &[arg.clone(), other.clone()]), Value::Number(4.0));
    assert_eq!(n(&mut w, std::slice::from_ref(&arg)), Value::Number(1.0));
    drop(keep);
}
