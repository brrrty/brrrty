//! Filters written in Rhai. The JavaScript behavior these mirror was probed
//! with Node 26 and 11ty 3.1.8, and each expected value below is what that
//! probe printed. The ports are generic: they stand for what a site's
//! `addFilter` calls do, with no site content.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brrrty_config::{CONFIG_FILE, Clock, SiteConfig, load_with};
use brrrty_core::{BuildOptions, Problem, build, codes};
use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, Loader, Tz};

/// 2023-11-14T12:00:00Z. Noon, so every time zone agrees on the date.
const NOON_MS: i64 = 1_699_963_200_000;

fn load(script: &str) -> SiteConfig {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join(CONFIG_FILE), script).unwrap();
    match load_with(t.path(), Clock::Fixed(NOON_MS)) {
        Ok(Some(c)) => c,
        Ok(None) => panic!("no config"),
        Err(p) => panic!("config failed: {p:#?}"),
    }
}

fn load_err(script: &str) -> Problem {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join(CONFIG_FILE), script).unwrap();
    match load_with(t.path(), Clock::Fixed(NOON_MS)) {
        Err(mut p) => {
            assert_eq!(p.len(), 1, "{p:#?}");
            p.remove(0)
        }
        Ok(_) => panic!("expected a problem"),
    }
}

struct NoPartials;
impl Loader for NoPartials {
    fn load(&self, _: &str) -> Result<(String, String), brrrty_liquid::LoadError> {
        Err(Default::default())
    }
}

fn engine(cfg: &SiteConfig) -> Engine {
    let mut e = Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    });
    e.register_eleventy_filters();
    for f in &cfg.filters {
        e.register_args_filter(&f.name, f.run.clone());
    }
    e
}

fn data(json: &str) -> Value {
    Value::from_json(&serde_json::from_str(json).unwrap())
}

/// Render a Liquid template with `json` as its data.
fn render(cfg: &SiteConfig, tpl: &str, json: &str) -> Result<String, String> {
    let e = engine(cfg);
    let t = e.parse("t.liquid", tpl).map_err(|e| e.to_string())?;
    e.render(&t, &data(json), &NoPartials)
        .map_err(|e| e.to_string())
}

/// What a script expression evaluates to, as data.
fn eval(expr: &str) -> Value {
    let cfg = load(&format!("config.add_global_data(\"r\", {expr});"));
    cfg.global_data["r"].clone()
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

fn keys(v: &Value) -> Vec<String> {
    match v {
        Value::Object(o) => o.keys().cloned().collect(),
        other => panic!("not an object: {other:?}"),
    }
}

// ---- JS key order ----

#[test]
fn object_keeps_insertion_order_and_maps_are_sorted() {
    let v = eval(r#"{ let o = object(); o.z = 1; o["a"] = 2; o.m = 3; o }"#);
    assert_eq!(keys(&v), ["z", "a", "m"]);
    // A plain map literal is a sorted map: Rhai's own behavior.
    let v = eval("#{ z: 1, a: 2, m: 3 }");
    assert_eq!(keys(&v), ["a", "m", "z"]);
}

#[test]
fn object_iterates_in_javascript_order() {
    // JS puts integer-like keys first, ascending, then the rest in
    // insertion order.
    let v = eval(
        r#"{
            let o = object();
            o.b = 1; o["10"] = 2; o.a = 3; o["2"] = 4; o["01"] = 5;
            let seen = [];
            for k in o { seen.push(k); }
            #{ keys: o.keys(), seen: seen, values: o.values(), len: o.len() }
        }"#,
    );
    let Value::Object(o) = v else { panic!() };
    let want = Value::from_json(&serde_json::json!(["2", "10", "b", "a", "01"]));
    assert_eq!(o["keys"], want);
    assert_eq!(o["seen"], want);
    assert_eq!(
        o["values"],
        Value::from_json(&serde_json::json!([4, 2, 1, 3, 5]))
    );
    assert_eq!(o["len"], Value::Number(5.0));
}

#[test]
fn object_converts_from_a_map_and_reads_missing_keys_as_unit() {
    let v =
        eval(r#"{ let o = object(#{ b: 1, a: 2 }); o.c = 3; #{ k: o.keys(), x: o.nope == () } }"#);
    let Value::Object(o) = v else { panic!() };
    assert_eq!(
        o["k"],
        Value::from_json(&serde_json::json!(["a", "b", "c"]))
    );
    assert_eq!(o["x"], Value::Bool(true));
}

#[test]
fn nested_objects_convert_in_insertion_order() {
    let v =
        eval(r#"{ let o = object(); o.z = object(); o.z.y = 1; o.z.b = 2; o.a = [object()]; o }"#);
    assert_eq!(keys(&v), ["z", "a"]);
    let Value::Object(o) = &v else { panic!() };
    assert_eq!(keys(&o["z"]), ["y", "b"]);
}

#[test]
fn data_from_brrrty_keeps_its_order_through_a_filter() {
    let cfg = load(
        r#"config.add_filter("pass", |v| v);
           config.add_filter("keys", |v| v.keys());
           config.add_filter("add", |v| { v.last = 1; v });"#,
    );
    let json = r#"{"o":{"z":1,"a":{"y":1,"b":2},"m":3}}"#;
    assert_eq!(
        render(&cfg, "{{ o | pass | json }}", json).unwrap(),
        r#"{"z":1,"a":{"y":1,"b":2},"m":3}"#
    );
    assert_eq!(
        render(&cfg, "{{ o | add | json }}", json).unwrap(),
        r#"{"z":1,"a":{"y":1,"b":2},"m":3,"last":1}"#
    );
    assert_eq!(
        render(&cfg, "{{ o | keys | json }}", json).unwrap(),
        r#"["z","a","m"]"#
    );
}

// ---- arguments ----

#[test]
fn a_missing_argument_is_unit_and_extra_ones_are_ignored() {
    let cfg = load(
        r#"config.add_filter("dflt", |v, n| `${v}:${n ?? 40}`);
           config.add_filter("one", |v| v);"#,
    );
    assert_eq!(render(&cfg, "{{ 'x' | dflt }}", "{}").unwrap(), "x:40");
    assert_eq!(render(&cfg, "{{ 'x' | dflt: 7 }}", "{}").unwrap(), "x:7");
    assert_eq!(render(&cfg, "{{ 'x' | one: 7, 8 }}", "{}").unwrap(), "x");
}

#[test]
fn keyword_arguments_arrive_in_place_as_name_value_pairs() {
    // 11ty 3.1.8 (LiquidJS): `{{ "x" | f: 1, k: 2, 3 }}` calls
    // f("x", 1, ["k", 2], 3); a quoted name keeps its quotes; `k:` alone
    // is ["k", null]; a repeated name is passed twice.
    let cfg = load(r#"config.add_filter("f", |v, a, b, c| [v, a, b, c]);"#);
    let r = |t: &str| render(&cfg, t, "{}").unwrap();
    assert_eq!(r("{{ 'x' | f | json }}"), r#"["x",null,null,null]"#);
    assert_eq!(r("{{ 'x' | f: 1, 2 | json }}"), r#"["x",1,2,null]"#);
    assert_eq!(
        r("{{ 'x' | f: k: 2 | json }}"),
        r#"["x",["k",2],null,null]"#
    );
    assert_eq!(
        r("{{ 'x' | f: 1, k: 2, 3 | json }}"),
        r#"["x",1,["k",2],3]"#
    );
    assert_eq!(
        r("{{ 'x' | f: \"k\": 2 | json }}"),
        r#"["x",["\"k\"",2],null,null]"#
    );
    assert_eq!(
        r("{{ 'x' | f: k: | json }}"),
        r#"["x",["k",null],null,null]"#
    );
    assert_eq!(
        r("{{ 'x' | f: a: 1, a: 2 | json }}"),
        r#"["x",["a",1],["a",2],null]"#
    );
}

#[test]
fn a_config_filter_replaces_a_standard_one() {
    let cfg = load(r#"config.add_filter("upcase", |v| `<${v}>`);"#);
    assert_eq!(render(&cfg, "{{ 'a' | upcase }}", "{}").unwrap(), "<a>");
}

#[test]
fn the_last_filter_with_a_name_wins() {
    let cfg = load(
        r#"config.add_filter("f", |v| "one");
           config.add_filter("f", |v| "two");"#,
    );
    assert_eq!(render(&cfg, "{{ 1 | f }}", "{}").unwrap(), "two");
}

#[test]
fn dates_and_numbers_come_in_as_javascript_values() {
    let cfg = load(
        r#"config.add_filter("ms", |d| get_time(d));
           config.add_filter("kind", |v| type_of(v));"#,
    );
    let e = engine(&cfg);
    let t = e
        .parse(
            "t.liquid",
            "{{ d | ms }}|{{ n | kind }}|{{ s | kind }}|{{ a | kind }}|{{ o | kind }}",
        )
        .unwrap();
    let d = {
        let mut m = indexmap::IndexMap::new();
        m.insert(
            "d".to_string(),
            brrrty_config::date_value(NOON_MS as f64).unwrap(),
        );
        m.insert("n".into(), Value::Number(1.5));
        m.insert("s".into(), s("x"));
        m.insert("a".into(), data("[1]"));
        m.insert("o".into(), data("{}"));
        Value::Object(Arc::new(m))
    };
    assert_eq!(
        e.render(&t, &d, &NoPartials).unwrap(),
        format!("{NOON_MS}|f64|string|array|object")
    );
}

// ---- errors ----

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn site(script: &str, pages: &[(&str, &str)]) -> (tempfile::TempDir, brrrty_core::BuildReport) {
    let t = tempfile::tempdir().unwrap();
    write(t.path(), CONFIG_FILE, script);
    for (name, body) in pages {
        write(t.path(), &format!("src/{name}"), body);
    }
    let cfg = load_with(t.path(), Clock::Fixed(NOON_MS)).unwrap().unwrap();
    let opts = BuildOptions {
        root: t.path().to_path_buf(),
        input: PathBuf::from("src"),
        output: PathBuf::from("_site"),
        config: cfg.build_options(),
        ..Default::default()
    };
    let report = build(&opts, &brrrty_markdown::MarkdownIt::new());
    (t, report)
}

#[test]
fn a_failing_filter_refuses_its_page_with_both_positions() {
    let (t, r) = site(
        "config.add_filter(\"ok\", |v| v);\nconfig.add_filter(\"boom\", |v| {\n    throw \"it broke\";\n});\n",
        &[
            ("good.liquid", "fine"),
            ("bad.liquid", "line one\n  {{ 'x' | boom }}\n"),
        ],
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/good/index.html")).unwrap(),
        "fine"
    );
    assert!(!t.path().join("_site/bad/index.html").exists());
    let errors: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.severity == brrrty_core::Severity::Error)
        .collect();
    assert_eq!(errors.len(), 1, "{:?}", r.problems);
    let p = errors[0];
    assert_eq!(p.code, codes::LIQUID_RENDER);
    // The template's position...
    assert_eq!(p.path.as_deref(), Some(Path::new("src/bad.liquid")));
    assert_eq!((p.line, p.column), (Some(2), Some(3)));
    // ...and the script's own, in the message.
    assert!(p.message.contains("it broke"), "{}", p.message);
    assert!(
        p.message.contains("brrrty.config.rhai:3:5"),
        "{}",
        p.message
    );
    assert!(p.message.contains("`boom`"), "{}", p.message);
}

#[test]
fn a_result_that_cannot_be_data_refuses_the_page() {
    let (t, r) = site(
        "config.add_filter(\"fnptr\", |v| |x| x);\n",
        &[("a.liquid", "{{ 1 | fnptr }}"), ("b.liquid", "b")],
    );
    assert!(t.path().join("_site/b/index.html").exists());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::LIQUID_RENDER)
        .unwrap();
    assert!(p.message.contains("fnptr"), "{}", p.message);
    assert!(p.message.contains("can't be data"), "{}", p.message);
}

#[test]
fn a_runaway_filter_is_stopped() {
    let (_t, r) = site(
        "config.add_filter(\"spin\", |v| { loop {} });\n",
        &[("a.liquid", "{{ 1 | spin }}"), ("b.liquid", "b")],
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::LIQUID_RENDER)
        .unwrap();
    // Whichever limit comes first: operations, or the time cap.
    assert!(
        p.message.contains("operations") || p.message.contains("seconds"),
        "{}",
        p.message
    );
}

#[test]
fn a_nondeterministic_filter_refuses_only_the_pages_that_use_it() {
    let (t, r) = site(
        "config.add_nondeterministic_filter(\"shuffle\");\n",
        &[
            ("a.liquid", "{{ 'x' | shuffle }}"),
            ("b.liquid", "no shuffle here"),
        ],
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/b/index.html")).unwrap(),
        "no shuffle here"
    );
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::LIQUID_RENDER)
        .unwrap();
    assert!(p.message.contains("nondeterministic"), "{}", p.message);
    assert!(p.message.contains("shuffle"), "{}", p.message);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.liquid")));
}

#[test]
fn add_filter_needs_a_function() {
    let p = load_err("config.add_filter(\"x\", 5);");
    assert_eq!(p.code, codes::CONFIG_ERROR);
    assert_eq!(p.line, Some(1));
}

// ---- regex ----

#[test]
fn regex_replace_follows_javascript() {
    // Each expected value is Node's `"..".replace(/../flags, "..")`.
    let r = |s: &str, pat: &str, rep: &str, flags: &str| {
        let src = format!(
            "regex_replace({}, {}, {}, {})",
            json(s),
            json(pat),
            json(rep),
            json(flags)
        );
        eval(&src)
    };
    assert_eq!(r("a-b-c", "-", "+", "g"), s("a+b+c"));
    assert_eq!(r("a-b-c", "-", "+", ""), s("a+b-c"));
    assert_eq!(
        r(
            "john smith",
            r"(\w+) (\w+)",
            "$2, $1 [$&] $$ $` $' $3 $0 $<x>",
            ""
        ),
        s("smith, john [john smith] $   $3 $0 $<x>")
    );
    assert_eq!(
        r("john smith", r"(?<f>\w+) (?<l>\w+)", "$<l> $<f> $<zz>", ""),
        s("smith john ")
    );
    assert_eq!(r("abc", "(b)", "$01|$10|$1a", ""), s("ab|b0|bac"));
    assert_eq!(r("AbC", "b", "x", "i"), s("AxC"));
    assert_eq!(r("a\nb", "^", ">", "gm"), s(">a\n>b"));
    assert_eq!(r("ab", "(?:)", "-", "g"), s("-a-b-"));
    assert_eq!(r("aaa", "a", "$&$&", "g"), s("aaaaaa"));
    assert_eq!(r("x", "x", "$", ""), s("$"));
    assert_eq!(r("x", "x", "$$$", ""), s("$$"));
    assert_eq!(r("ab", "(a)|(z)", "[$2]", ""), s("[]b"));
    assert_eq!(r("a b", r"[\s]", "_", "g"), s("a_b"));
    assert_eq!(r("a-b", r"[\w-]+", "X", ""), s("X"));
}

#[test]
fn regex_classes_are_javascripts_not_unicode() {
    let r = |s: &str, pat: &str, rep: &str, flags: &str| {
        eval(&format!(
            "regex_replace({}, {}, {}, {})",
            json(s),
            json(pat),
            json(rep),
            json(flags)
        ))
    };
    // \d and \w are ASCII only; `.` stops at \r; the `s` flag lets it cross.
    assert_eq!(r("\u{663}x3", r"\d", "#", "g"), s("\u{663}x#"));
    assert_eq!(r("é a", r"\w", "#", "g"), s("é #"));
    assert_eq!(r("é a", r"\b", "|", "g"), s("é |a|"));
    assert_eq!(r("a\nb\rc", ".", "#", "g"), s("#\n#\r#"));
    assert_eq!(r("a\nb", ".", "#", "gs"), s("###"));
}

#[test]
fn regex_split_follows_javascript() {
    let sp = |text: &str, pat: &str| eval(&format!("regex_split({}, {})", json(text), json(pat)));
    let j = |v: serde_json::Value| Value::from_json(&v);
    assert_eq!(
        sp("a1b2", r"(\d)"),
        j(serde_json::json!(["a", "1", "b", "2", ""]))
    );
    assert_eq!(sp("abc", "(?:)"), j(serde_json::json!(["a", "b", "c"])));
    assert_eq!(sp("a,b,,c", ","), j(serde_json::json!(["a", "b", "", "c"])));
    assert_eq!(sp("", ","), j(serde_json::json!([""])));
    assert_eq!(sp("", "(?:)"), j(serde_json::json!([])));
    assert_eq!(sp("abc", "b*"), j(serde_json::json!(["a", "c"])));
    assert_eq!(sp(",a", ","), j(serde_json::json!(["", "a"])));
    // An unmatched group is `undefined`, which Rhai sees as ().
    let Value::Array(a) = sp("a b", r"(x)?\s") else {
        panic!()
    };
    assert_eq!(a.len(), 3);
    assert_eq!(a[1], Value::Undefined);
    // \s is JavaScript's: it has the BOM and not U+0085.
    assert_eq!(
        sp("a b\u{feff}c\u{85}d", r"\s+"),
        j(serde_json::json!(["a", "b", "c\u{85}d"]))
    );
}

#[test]
fn regex_test_matches_anywhere() {
    assert_eq!(eval(r#"regex_test("a12", "\\d+")"#), Value::Bool(true));
    assert_eq!(eval(r#"regex_test("a12", "^\\d+$")"#), Value::Bool(false));
    assert_eq!(eval(r#"regex_test("ABC", "b", "i")"#), Value::Bool(true));
}

#[test]
fn unsupported_regex_features_are_refused_with_a_line_number() {
    for (pat, what) in [
        ("(?=a)b", "lookahead"),
        ("(?<=a)b", "lookbehind"),
        (r"(a)\1", "backreference"),
        ("(", "unclosed"),
    ] {
        let p = load_err(&format!(
            "let x = 1;\nlet y = regex_test(\"s\", {});\n",
            json(pat)
        ));
        assert_eq!(p.code, codes::CONFIG_INVALID, "{pat}");
        assert_eq!(p.line, Some(2), "{pat}: {p:?}");
        assert!(p.message.contains(pat), "{pat}: {}", p.message);
        let _ = what;
    }
    let p = load_err("regex_replace(\"s\", \"(?=a)\", \"\", \"\");");
    assert!(p.message.contains("lookaround"), "{p:?}");
}

fn json(s: &str) -> String {
    serde_json::to_string(s).unwrap()
}

// ---- ports of real filters (generic: they stand for a site's addFilter) ----

const EXCERPT: &str = r#"
config.add_filter("excerptWords", |s, word_count| {
    if !truthy(s) { return ""; }
    let n = word_count ?? 40;
    let clean = regex_replace(s, "(<([^>]+)>)", "", "gi");
    let words = regex_split(clean, "\\s+");
    if words.len() <= n { return clean; }
    if n < 1 { throw "Cannot read properties of undefined (reading 'replace')"; }
    let kept = words.extract(0, n.to_int());
    let last = kept.len() - 1;
    kept[last] = regex_replace(kept[last], "[.,!?;:]$", "", "");
    kept.reduce(|acc, w, i| if i == 0 { w } else { acc + " " + w }, "")
});
"#;

#[test]
fn excerpt_words_matches_javascript() {
    // Node: excerptWords(str, n) for each row.
    let cfg = load(EXCERPT);
    let r = |tpl: &str, json: &str| render(&cfg, tpl, json);
    let html = r#"{"h":"<p>Hello, <b>big</b> world. Again!</p>"}"#;
    assert_eq!(r("{{ h | excerptWords: 2 }}", html).unwrap(), "Hello, big");
    assert_eq!(
        r("{{ h | excerptWords: 3 }}", html).unwrap(),
        "Hello, big world"
    );
    assert_eq!(
        r("{{ h | excerptWords }}", html).unwrap(),
        "Hello, big world. Again!"
    );
    assert_eq!(r("{{ u | excerptWords: 3 }}", "{}").unwrap(), "");
    assert_eq!(r("{{ u | excerptWords: 3 }}", r#"{"u":""}"#).unwrap(), "");
    assert_eq!(
        r(
            "{{ u | excerptWords: 2 }}",
            r#"{"u":"  lead  and\ttrail  "}"#
        )
        .unwrap(),
        " lead"
    );
    assert_eq!(
        r("{{ u | excerptWords: 2.5 }}", r#"{"u":"a b c d"}"#).unwrap(),
        "a b"
    );
    // JS throws a TypeError for a count of 0; so does the port.
    let e = r("{{ u | excerptWords: 0 }}", r#"{"u":"one two"}"#).unwrap_err();
    assert!(e.contains("Cannot read properties of undefined"), "{e}");
}

#[test]
#[ignore = "needs the markdown helper from Task 3"]
fn markdown_filter_is_a_stub_until_the_helper_exists() {
    let cfg = load(r#"config.add_filter("markdown", |s| markdown(s ?? ""));"#);
    assert_eq!(
        render(&cfg, "{{ '*a*' | markdown }}", "{}").unwrap(),
        "<p><em>a</em></p>\n"
    );
}

const COLLECTION_FILTERS: &str = r#"
config.add_filter("published", |games| {
    if !truthy(games) { return []; }
    games.filter(|g| g.status == "published")
});
config.add_filter("byCategory", |games, slug| {
    if !truthy(games) { return []; }
    games.filter(|g| g.category == slug)
});
config.add_nondeterministic_filter("shuffle");
"#;

#[test]
fn published_and_by_category_match_javascript() {
    let cfg = load(COLLECTION_FILTERS);
    let games = r#"{"games":[
        {"status":"published","category":"a","n":1},
        {"status":"draft","category":"a","n":2},
        {"status":"published","category":"b","n":3}]}"#;
    assert_eq!(
        render(&cfg, "{{ games | published | json }}", games).unwrap(),
        r#"[{"status":"published","category":"a","n":1},{"status":"published","category":"b","n":3}]"#
    );
    assert_eq!(
        render(
            &cfg,
            "{{ games | byCategory: 'a' | map: 'n' | json }}",
            games
        )
        .unwrap(),
        "[1,2]"
    );
    assert_eq!(
        render(&cfg, "{{ nope | published | json }}", "{}").unwrap(),
        "[]"
    );
    assert_eq!(
        render(&cfg, "{{ nope | byCategory: 'a' | json }}", "{}").unwrap(),
        "[]"
    );
}

const STRING_FILTERS: &str = r#"
config.add_filter("domain", |url| {
    let h = url_hostname(url);
    if h == () { url } else { h }
});
config.add_filter("initials", |name| {
    let parts = regex_split(if truthy(name) { name.to_string() } else { "" }, "\\s+")
        .filter(|p| p != "");
    if parts.len() == 0 { return "?"; }
    let first = parts[0].sub_string(0, 1);
    let last = if parts.len() > 1 { parts[parts.len() - 1].sub_string(0, 1) } else { "" };
    (first + last).to_upper()
});
config.add_filter("endsWith", |s, suffix| {
    let t = if truthy(s) { s.to_string() } else { "" };
    t.ends_with(suffix)
});
config.add_filter("objectLength", |obj| {
    if truthy(obj) { obj.len() } else { 0 }
});
config.add_filter("now", || get_time(now()));
config.add_filter("date", |ts| to_locale_date_string(date(ts)));
"#;

#[test]
fn truthy_is_javascripts() {
    // `null` and `undefined` are both `()` in Rhai, so `()` is falsy.
    for (expr, want) in [
        ("()", false),
        ("false", false),
        ("0", false),
        ("0.0", false),
        ("-0.0", false),
        ("0.0 / 0.0", false),
        ("\"\"", false),
        ("true", true),
        ("1", true),
        ("\"0\"", true),
        ("\" \"", true),
        ("[]", true),
        ("#{}", true),
        ("object()", true),
        ("now()", true),
    ] {
        assert_eq!(
            eval(&format!("truthy({expr})")),
            Value::Bool(want),
            "{expr}"
        );
    }
}

#[test]
fn string_filters_match_javascript() {
    let cfg = load(STRING_FILTERS);
    let r = |tpl: &str, json: &str| render(&cfg, tpl, json).unwrap();
    for (input, want) in [
        ("https://Sub.Example.com:8080/a?b", "sub.example.com"),
        ("not a url", "not a url"),
        ("", ""),
        ("mailto:a@b.c", ""),
        ("http://[::1]:3000/", "[::1]"),
        ("https://bücher.de/x", "xn--bcher-kva.de"),
        ("//x.com", "//x.com"),
        ("file:///tmp/a", ""),
    ] {
        assert_eq!(
            r("{{ u | domain }}", &format!(r#"{{"u":{}}}"#, json(input))),
            want,
            "{input}"
        );
    }
    assert_eq!(r("{{ u | domain }}", "{}"), "");

    for (input, want) in [
        ("ada lovelace", "AL"),
        ("  grace  brewster murray hopper ", "GH"),
        ("x", "X"),
        ("", "?"),
        ("ßeta ünder", "SSÜ"),
    ] {
        assert_eq!(
            r("{{ n | initials }}", &format!(r#"{{"n":{}}}"#, json(input))),
            want,
            "{input}"
        );
    }
    assert_eq!(r("{{ n | initials }}", "{}"), "?");
    assert_eq!(r("{{ n | initials }}", r#"{"n":0}"#), "?");
    assert_eq!(r("{{ 0 | endsWith: '0' }}", "{}"), "false");

    assert_eq!(r("{{ 'abc.xml' | endsWith: '.xml' }}", "{}"), "true");
    assert_eq!(r("{{ u | endsWith: 'x' }}", "{}"), "false");
    assert_eq!(r("{{ '' | endsWith: '' }}", "{}"), "true");

    assert_eq!(r("{{ o | objectLength }}", r#"{"o":{"a":1,"b":2}}"#), "2");
    assert_eq!(r("{{ o | objectLength }}", "{}"), "0");
    assert_eq!(r("{{ o | objectLength }}", r#"{"o":{}}"#), "0");
    assert_eq!(r("{{ o | objectLength }}", r#"{"o":[1,2,3]}"#), "3");
    assert_eq!(r("{{ o | objectLength }}", r#"{"o":0}"#), "0");

    // `Date.now()` under SOURCE_DATE_EPOCH.
    assert_eq!(r("{{ 1 | now }}", "{}"), NOON_MS.to_string());
    // toLocaleDateString('en-US', { year, month: 'long', day }), probed
    // with Node. Noon UTC, so any time zone gives the same date.
    assert_eq!(
        r("{{ t | date }}", &format!(r#"{{"t":{NOON_MS}}}"#)),
        "November 14, 2023"
    );
    assert_eq!(r("{{ t | date }}", "{}"), "Invalid Date");
}
