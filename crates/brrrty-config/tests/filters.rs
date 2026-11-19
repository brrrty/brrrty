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
    // It points at the way to port the filter as it is.
    assert!(p.message.contains("add_random_filter"), "{}", p.message);
    assert_eq!(p.path.as_deref(), Some(Path::new("src/a.liquid")));
}

/// A port of the usual JavaScript Fisher-Yates shuffle:
///
/// ```js
/// const a = (arr || []).slice();
/// for (let i = a.length - 1; i > 0; i--) {
///   const j = Math.floor(Math.random() * (i + 1));
///   [a[i], a[j]] = [a[j], a[i]];
/// }
/// return a;
/// ```
const SHUFFLE: &str = r#"
config.add_random_filter("shuffle", |arr| {
    let a = if truthy(arr) { arr } else { [] };
    let i = a.len() - 1;
    while i > 0 {
        let j = (random() * (i + 1)).floor().to_int();
        let t = a[i];
        a[i] = a[j];
        a[j] = t;
        i -= 1;
    }
    a
});
"#;

fn notes(r: &brrrty_core::BuildReport) -> Vec<&Problem> {
    r.problems
        .iter()
        .filter(|p| p.code == codes::RANDOM_OUTPUT)
        .collect()
}

#[test]
fn a_random_filter_builds_its_pages_with_a_note_each() {
    let (t, r) = site(
        SHUFFLE,
        &[
            (
                "a.liquid",
                "{{ '1,2,3,4,5' | split: ',' | shuffle | join: ',' }}",
            ),
            (
                "b.njk",
                "{% set s = [1, 2, 3, 4, 5] | shuffle %}{{ s | sort | join(',') }}|{{ s.length }}",
            ),
            ("c.liquid", "{{ nothing | shuffle | size }}"),
            ("d.liquid", "no shuffle here"),
        ],
    );
    assert!(
        r.problems
            .iter()
            .all(|p| p.severity == brrrty_core::Severity::Info),
        "{:#?}",
        r.problems
    );
    let read = |rel: &str| fs::read_to_string(t.path().join("_site").join(rel)).unwrap();
    // Every result is a permutation of the input.
    let mut a: Vec<String> = read("a/index.html").split(',').map(String::from).collect();
    a.sort();
    assert_eq!(a, ["1", "2", "3", "4", "5"]);
    assert_eq!(read("b/index.html"), "1,2,3,4,5|5");
    assert_eq!(read("c/index.html"), "0");
    assert_eq!(read("d/index.html"), "no shuffle here");
    // One note per page that calls it, at the page; none for the others.
    let mut at: Vec<&Path> = notes(&r).iter().filter_map(|p| p.path.as_deref()).collect();
    at.sort();
    assert_eq!(
        at,
        [
            Path::new("src/a.liquid"),
            Path::new("src/b.njk"),
            Path::new("src/c.liquid")
        ]
    );
    let n = notes(&r)[0];
    assert_eq!(n.severity, brrrty_core::Severity::Info);
    assert!(n.message.contains("`shuffle`"), "{}", n.message);
    assert!(
        n.message
            .contains("11ty writes it differently on every build"),
        "{}",
        n.message
    );
    assert!(
        n.help.as_deref().unwrap().contains("BRRRTY_RANDOM_SEED"),
        "{:?}",
        n.help
    );
}

/// Whether the page written to `output` is random, and its path random.
fn random_page(r: &brrrty_core::BuildReport, output: &str) -> (bool, bool) {
    let p = r
        .pages
        .iter()
        .find(|p| p.output == Path::new(output))
        .unwrap_or_else(|| panic!("{output} not written: {:#?}", r.pages));
    (p.random, p.random_path)
}

fn note_for<'a>(r: &'a brrrty_core::BuildReport, path: &str) -> Option<&'a Problem> {
    notes(r)
        .into_iter()
        .find(|p| p.path.as_deref() == Some(Path::new(path)))
}

const PICK: &str =
    "config.add_random_filter(\"pick\", |a| a[(random() * a.len()).floor().to_int()]);\n";

#[test]
fn a_page_that_shows_random_content_is_random_too() {
    let (_t, r) = site(
        &format!("{SHUFFLE}{PICK}"),
        &[
            (
                "post.liquid",
                "---\ntags: post\n---\n{{ '1,2,3' | split: ',' | shuffle | join: '' }}",
            ),
            ("plain.liquid", "---\ntags: post\n---\nplain"),
            // Shows the random post's content.
            (
                "index.liquid",
                "{% for p in collections.post %}[{{ p.templateContent }}]{% endfor %}",
            ),
            (
                "index2.njk",
                "{% for p in collections.post %}[{{ p.templateContent | safe }}]{% endfor %}",
            ),
            // Shows only data and urls, which no random filter made.
            (
                "urls.liquid",
                "{% for p in collections.post %}{{ p.url }}{% endfor %}",
            ),
            // Shows the content of a page that shows random content.
            (
                "outer.liquid",
                "---\ntags: outer\n---\n{% for p in collections.post %}[{{ p.templateContent }}]{% endfor %}",
            ),
            (
                "outermost.liquid",
                "{% for p in collections.outer %}{{ p.templateContent }}{% endfor %}",
            ),
        ],
    );
    assert!(
        r.problems
            .iter()
            .all(|p| p.severity == brrrty_core::Severity::Info),
        "{:#?}",
        r.problems
    );
    assert_eq!(random_page(&r, "post/index.html"), (true, false));
    assert_eq!(random_page(&r, "plain/index.html"), (false, false));
    assert_eq!(random_page(&r, "urls/index.html"), (false, false));
    for (src, out) in [
        ("src/index.liquid", "index.html"),
        ("src/index2.njk", "index2/index.html"),
        ("src/outer.liquid", "outer/index.html"),
        ("src/outermost.liquid", "outermost/index.html"),
    ] {
        assert_eq!(random_page(&r, out), (true, false), "{out}");
        let n = note_for(&r, src).unwrap_or_else(|| panic!("no note for {src}"));
        assert!(n.message.contains("shows the content of"), "{}", n.message);
    }
    assert!(note_for(&r, "src/urls.liquid").is_none());
    assert!(note_for(&r, "src/plain.liquid").is_none());
}

#[test]
fn a_random_permalink_makes_every_reader_of_collections_random() {
    let (_t, r) = site(
        PICK,
        &[
            (
                "perma.liquid",
                "---\npermalink: \"/p-{{ 'a,b' | split: ',' | pick }}/\"\ntags: post\n---\nperma",
            ),
            (
                "urls.liquid",
                "{% for p in collections.post %}{{ p.url }}{% endfor %}",
            ),
            ("alone.liquid", "no collections here"),
            (
                "list.njk",
                "---\npagination:\n  data: collections.post\n  size: 1\n---\n{{ pagination.items[0].url }}",
            ),
        ],
    );
    assert!(
        r.problems
            .iter()
            .all(|p| p.severity == brrrty_core::Severity::Info),
        "{:#?}",
        r.problems
    );
    let perma = r
        .pages
        .iter()
        .find(|p| p.source == Path::new("src/perma.liquid"))
        .unwrap();
    assert!(perma.random && perma.random_path);
    assert_eq!(random_page(&r, "urls/index.html"), (true, false));
    // Its page comes from the collections, so its path may be random too.
    assert_eq!(random_page(&r, "list/index.html"), (true, true));
    assert_eq!(random_page(&r, "alone/index.html"), (false, false));
    let n = note_for(&r, "src/urls.liquid").unwrap();
    assert!(n.message.contains("reads `collections`"), "{}", n.message);
    assert!(n.message.contains("src/perma.liquid"), "{}", n.message);
    assert!(note_for(&r, "src/alone.liquid").is_none());
}

#[test]
fn only_the_pages_that_draw_of_a_paginated_template_are_random() {
    let (_t, r) = site(
        SHUFFLE,
        &[(
            "list.liquid",
            "---\npagination:\n  data: items\n  size: 1\nitems: [a, b]\n---\n{% if pagination.pageNumber == 0 %}{{ '1,2,3' | split: ',' | shuffle | join: '' }}{% else %}fixed{% endif %}",
        )],
    );
    assert_eq!(random_page(&r, "list/index.html"), (true, false));
    assert_eq!(random_page(&r, "list/1/index.html"), (false, false));
    assert_eq!(notes(&r).len(), 1);
}

#[test]
fn a_page_gets_one_note_however_many_random_filters_it_calls() {
    let (_t, r) = site(
        &format!("{SHUFFLE}{PICK}"),
        &[(
            "a.liquid",
            "{{ '1,2' | split: ',' | shuffle | join: '' }}{{ '1,2' | split: ',' | pick }}{{ '1,2' | split: ',' | pick }}",
        )],
    );
    let all = notes(&r);
    assert_eq!(all.len(), 1, "{all:#?}");
    assert!(
        all[0].message.contains("`pick` and `shuffle`"),
        "{}",
        all[0].message
    );
}

#[test]
fn a_refused_page_gets_no_random_note() {
    let (t, r) = site(
        &format!(
            "{SHUFFLE}fn mk(v) {{ |x| random() }}\nconfig.add_random_filter(\"mk\", Fn(\"mk\"));\n"
        ),
        &[
            // The filter fails: its result can't be data.
            ("a.liquid", "{{ 1 | mk }}"),
            // Refused for another reason after a random filter ran.
            (
                "b.liquid",
                "{{ '1,2' | split: ',' | shuffle | join: '' }}{{ 1 | nope }}",
            ),
        ],
    );
    assert!(!t.path().join("_site/a/index.html").exists());
    assert!(!t.path().join("_site/b/index.html").exists());
    assert!(notes(&r).is_empty(), "{:#?}", r.problems);
}

#[test]
fn a_random_filter_draws_every_permutation() {
    // 3 items have 6 orders; 200 draws miss one with odds below 1 in 10^15.
    let cfg = load(SHUFFLE);
    let tpl = (0..200)
        .map(|_| "{{ '1,2,3' | split: ',' | shuffle | join: '' }} ")
        .collect::<String>();
    let out = render(&cfg, &tpl, "{}").unwrap();
    let seen: std::collections::BTreeSet<&str> = out.split_whitespace().collect();
    assert_eq!(
        seen.into_iter().collect::<Vec<_>>(),
        ["123", "132", "213", "231", "312", "321"]
    );
}

#[test]
fn random_works_only_inside_a_random_filter() {
    // A plain filter: its page is refused, with the way out.
    let (t, r) = site(
        "config.add_filter(\"roll\", |v| random());\n",
        &[("a.liquid", "{{ 1 | roll }}"), ("b.liquid", "b")],
    );
    assert!(t.path().join("_site/b/index.html").exists());
    assert!(!t.path().join("_site/a/index.html").exists());
    let p = r
        .problems
        .iter()
        .find(|p| p.code == codes::LIQUID_RENDER)
        .unwrap();
    assert!(p.message.contains("add_random_filter"), "{}", p.message);
    assert!(notes(&r).is_empty(), "{:#?}", r.problems);
    // A collection.
    let (_t, r) = site(
        "config.add_collection(\"c\", |api| [random()]);\n",
        &[("a.liquid", "{{ collections.c | size }}")],
    );
    assert!(
        r.problems
            .iter()
            .any(|p| p.message.contains("add_random_filter")),
        "{:#?}",
        r.problems
    );
    // Global data, and the script's top level.
    let p = load_err("config.add_global_data(\"r\", random());");
    assert!(p.message.contains("add_random_filter"), "{}", p.message);
    // A random filter's own function, registered again as a plain filter.
    let (_t, r) = site(
        "fn roll(v) { random() }\nconfig.add_random_filter(\"r\", Fn(\"roll\"));\nconfig.add_filter(\"p\", Fn(\"roll\"));\n",
        &[("a.liquid", "{{ 1 | r }}"), ("b.liquid", "{{ 1 | p }}")],
    );
    let refused: Vec<_> = r
        .problems
        .iter()
        .filter(|p| p.code == codes::LIQUID_RENDER)
        .filter_map(|p| p.path.as_deref())
        .collect();
    assert_eq!(refused, [Path::new("src/b.liquid")]);
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
    // Emoji, CJK and combining marks: neither pattern can split a
    // surrogate pair, so text outside the BMP is fine.
    let post =
        r#"{"u":"<p>Ship it \ud83d\ude80 now, \u6f22\u5b57 e\u0301\ud83d\udc4d\ud83c\udffd!</p>"}"#;
    assert_eq!(
        r("{{ u | excerptWords: 4 }}", post).unwrap(),
        "Ship it \u{1F680} now"
    );
    assert_eq!(
        r("{{ u | excerptWords }}", post).unwrap(),
        "Ship it \u{1F680} now, \u{6F22}\u{5B57} e\u{301}\u{1F44D}\u{1F3FD}!"
    );
    assert_eq!(
        r(
            "{{ u | excerptWords: 1 }}",
            r#"{"u":"<b>\ud83d\ude00</b>\ud83d\ude00<i title=\"\ud83d\ude00\">x</i>"}"#
        )
        .unwrap(),
        "\u{1F600}\u{1F600}x"
    );
}

/// A filter's error names the function it happened in when the script
/// named one, and never the internal name of the filter's closure.
#[test]
fn filter_errors_name_only_functions_the_script_named() {
    let cfg = load(
        "fn strip(s) { regex_replace(s, \".\", \"\", \"g\") }\n\
         config.add_filter(\"direct\", |s| regex_replace(s, \".\", \"\", \"g\"));\n\
         config.add_filter(\"named\", |s| strip(s));",
    );
    let emoji = r#"{"u":"\ud83d\ude00"}"#;
    let e = render(&cfg, "{{ u | direct }}", emoji).unwrap_err();
    assert!(e.contains("outside the BMP"), "{e}");
    assert!(!e.contains("in function"), "{e}");
    assert!(!e.contains("anon"), "{e}");
    let e = render(&cfg, "{{ u | named }}", emoji).unwrap_err();
    assert!(e.contains("(in function `strip`)"), "{e}");
    assert!(!e.contains("anon"), "{e}");
}

/// The three filters a site written for markdown-it and Luxon needs. The expected values are
/// what Node 26 printed for `markdownIt()` (no options, so HTML is off) and Luxon 3.7.2.
const BUILTIN_FILTERS: &str = r#"
config.add_filter("markdown", |s| markdown(s));
config.add_filter("markdownify", |s| markdown_inline(s));
config.add_filter("utcDate", |d, format| luxon_format(d, format ?? "dd-LLL-yyyy", "utc"));
"#;

#[test]
fn markdown_filter_renders_with_html_off() {
    let cfg = load(BUILTIN_FILTERS);
    let r = |tpl: &str, json: &str| render(&cfg, tpl, json).unwrap();
    assert_eq!(r("{{ '*a*' | markdown }}", "{}"), "<p><em>a</em></p>\n");
    assert_eq!(
        r("{{ s | markdown }}", r#"{"s":"<b>x</b>"}"#),
        "<p>&lt;b&gt;x&lt;/b&gt;</p>\n"
    );
    // The indented code rule is on here (11ty's own instance turns it off).
    assert_eq!(
        r("{{ s | markdown }}", r#"{"s":"    code"}"#),
        "<pre><code>code\n</code></pre>\n"
    );
    assert_eq!(
        r("{{ s | markdown }}", r#"{"s":"a\n\n- x\n- y\n"}"#),
        "<p>a</p>\n<ul>\n<li>x</li>\n<li>y</li>\n</ul>\n"
    );
    assert_eq!(
        r("{{ s | markdown }}", r#"{"s":"~~s~~"}"#),
        "<p><s>s</s></p>\n"
    );
}

#[test]
fn markdownify_renders_inline_only() {
    let cfg = load(BUILTIN_FILTERS);
    let r = |json: &str| render(&cfg, "{{ s | markdownify }}", json).unwrap();
    assert_eq!(r(r#"{"s":"*a*"}"#), "<em>a</em>");
    assert_eq!(r(r#"{"s":"<b>x</b>"}"#), "&lt;b&gt;x&lt;/b&gt;");
    assert_eq!(r(r#"{"s":"    code"}"#), "    code");
    assert_eq!(r(r#"{"s":"a\n\n- x\n- y\n"}"#), "a\n\n- x\n- y\n");
    assert_eq!(r(r#"{"s":"x & y"}"#), "x &amp; y");
}

#[test]
fn markdown_of_a_falsy_value_is_empty_and_of_a_non_string_is_an_error() {
    let cfg = load(BUILTIN_FILTERS);
    for tpl in [
        "{{ nope | markdown }}",
        "{{ n | markdown }}",
        "{{ e | markdownify }}",
        "{{ f | markdownify }}",
    ] {
        let out = render(&cfg, tpl, r#"{"n":null,"e":"","f":false}"#).unwrap();
        assert_eq!(out, "", "{tpl}");
    }
    let err = render(&cfg, "{{ n | markdown }}", r#"{"n":5}"#).unwrap_err();
    assert!(err.contains("Input data should be a String"), "{err}");
}

/// `markdown(s, options)` is `markdownIt({ html, linkify, typographer })` with
/// `md.linkify.set({ fuzzyLink })`. Expected values are what Node printed (markdown-it 15.0.3).
#[test]
fn markdown_takes_markdown_it_options() {
    let cfg = load(
        r#"
config.add_filter("md", |s| markdown(s, #{ linkify: true, typographer: true, fuzzy_link: true }));
config.add_filter("strict", |s| markdown(s, #{ linkify: true }));
config.add_filter("raw", |s| { let o = object(); o.html = true; markdown(s, o) });
config.add_filter("typo", |s| markdown(s, #{ linkfy: true }));
config.add_filter("notbool", |s| markdown(s, #{ html: 1 }));
config.add_filter("notmap", |s| markdown(s, true));
"#,
    );
    let json = r#"{"s":"Founded Example.com -- \"the\" (c) https://x.io <b>x</b>"}"#;
    let r = |f: &str| render(&cfg, &format!("{{{{ s | {f} }}}}"), json);
    assert_eq!(
        r("md").unwrap(),
        "<p>Founded <a href=\"http://Example.com\">Example.com</a> – “the” © <a href=\"https://x.io\">https://x.io</a> &lt;b&gt;x&lt;/b&gt;</p>\n"
    );
    assert_eq!(
        r("strict").unwrap(),
        "<p>Founded Example.com -- &quot;the&quot; (c) <a href=\"https://x.io\">https://x.io</a> &lt;b&gt;x&lt;/b&gt;</p>\n"
    );
    assert_eq!(
        r("raw").unwrap(),
        "<p>Founded Example.com -- &quot;the&quot; (c) https://x.io <b>x</b></p>\n"
    );
    let e = r("typo").unwrap_err();
    assert!(e.contains("`linkfy`") && e.contains("fuzzy_link"), "{e}");
    let e = r("notbool").unwrap_err();
    assert!(e.contains("`html`") && e.contains("true or false"), "{e}");
    let e = r("notmap").unwrap_err();
    assert!(e.contains("options"), "{e}");
    // A falsy source is still "" with options.
    assert_eq!(render(&cfg, "{{ n | md }}", r#"{"n":null}"#).unwrap(), "");
}

fn render_date(cfg: &SiteConfig, tpl: &str, d: Value) -> Result<String, String> {
    let e = engine(cfg);
    let t = e.parse("t.liquid", tpl).map_err(|e| e.to_string())?;
    let mut m = indexmap::IndexMap::new();
    m.insert("d".to_string(), d);
    m.insert("txt".into(), s("2023-11-14"));
    e.render(&t, &Value::Object(Arc::new(m)), &NoPartials)
        .map_err(|e| e.to_string())
}

#[test]
fn utc_date_formats_like_luxon() {
    let cfg = load(BUILTIN_FILTERS);
    let d = brrrty_config::date_value(NOON_MS as f64).unwrap();
    let r = |tpl: &str| render_date(&cfg, tpl, d.clone()).unwrap();
    assert_eq!(r("{{ d | utcDate }}"), "14-Nov-2023");
    assert_eq!(r("{{ d | utcDate: 'yyyy-MM-dd' }}"), "2023-11-14");
    assert_eq!(r("{{ d | utcDate: 'DDDD' }}"), "Tuesday, November 14, 2023");
    assert_eq!(r("{{ d | utcDate: '' }}"), "");
    // A string is not a JS Date, so Luxon has no date to format.
    assert_eq!(r("{{ txt | utcDate }}"), "Invalid DateTime");
}

#[test]
fn luxon_format_refuses_other_locales_and_null_formats() {
    let cfg = load(
        r#"config.add_filter("fr", |d| luxon_format(d, "yyyy", "utc", "fr-FR"));
           config.add_filter("nul", |d| luxon_format(d, null_value(), "utc"));
           config.add_filter("local", |d| luxon_format(d, "yyyy", "local"));"#,
    );
    let d = brrrty_config::date_value(NOON_MS as f64).unwrap();
    let err = render_date(&cfg, "{{ d | fr }}", d.clone()).unwrap_err();
    assert!(err.contains("fr-FR") && err.contains("en-US"), "{err}");
    let err = render_date(&cfg, "{{ d | nul }}", d.clone()).unwrap_err();
    assert!(err.contains("reading 'length'"), "{err}");
    let err = render_date(&cfg, "{{ d | local }}", d).unwrap_err();
    assert!(err.contains("depends on the machine"), "{err}");
}

const COLLECTION_FILTERS: &str = r#"
config.add_filter("published", |games| {
    if !truthy(games) { return []; }
    games.filter(|g| g.status == "published")
});
config.add_filter("byCategory", |games, slug| {
    if !truthy(games) { return []; }
    let out = [];
    for g in games { if g.category == slug { out.push(g); } }
    out
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

// ---- fix round 1 ----

#[test]
fn null_survives_a_filter_as_a_scalar_an_object_key_and_an_array_item() {
    // 11ty prints `null` for each of these through an identity filter.
    let cfg = load(r#"config.add_filter("id", |v| v);"#);
    let r = |tpl: &str, json: &str| render(&cfg, tpl, json).unwrap();
    assert_eq!(r("{{ n | id | json }}", r#"{"n":null}"#), "null");
    assert_eq!(
        r(
            "{{ o | id | json }}",
            r#"{"o":{"a":null,"b":1,"c":{"d":null}}}"#
        ),
        r#"{"a":null,"b":1,"c":{"d":null}}"#
    );
    assert_eq!(
        r("{{ a | id | json }}", r#"{"a":[null,1,[null],{"k":null}]}"#),
        r#"[null,1,[null],{"k":null}]"#
    );
}

#[test]
fn null_is_unit_to_a_script_and_can_be_made_on_purpose() {
    let cfg = load(
        r#"config.add_filter("isnull", |v| v == ());
           config.add_filter("key", |o| o.k);
           config.add_filter("keyisnull", |o| o.k == ());
           config.add_filter("mk", |v| null_value());
           config.add_filter("setnull", |v| { let o = object(); o.a = null_value(); o.b = (); o });
           config.add_filter("falsy", |v| truthy(v));
           config.add_filter("copy", |o| { let c = object(); for k in o { c[k] = o[k]; } c });
           config.add_filter("copyq", |o| { let c = object(); c.q = o.n; c });
           config.add_filter("copyq", |o| { let c = object(); c.q = o.n; c });
           config.add_filter("cnst", |v| null_value());"#,
    );
    let r = |tpl: &str, json: &str| render(&cfg, tpl, json).unwrap();
    assert_eq!(r("{{ n | isnull }}", r#"{"n":null}"#), "true");
    assert_eq!(r("{{ n | falsy }}", r#"{"n":null}"#), "false");
    assert_eq!(r("{{ o | keyisnull }}", r#"{"o":{"k":null}}"#), "true");
    assert_eq!(r("{{ o | key | json }}", r#"{"o":{"k":null}}"#), "null");
    assert_eq!(r("{{ 1 | mk | json }}", "{}"), "null");
    assert_eq!(r("{{ 1 | cnst | json }}", "{}"), "null");
    // `o.b = ()` is `undefined`, which JSON drops.
    assert_eq!(r("{{ 1 | setnull | json }}", "{}"), r#"{"a":null}"#);
    // Copying key by key keeps the null, by index and by property.
    assert_eq!(
        r("{{ o | copy | json }}", r#"{"o":{"k":null,"j":1}}"#),
        r#"{"k":null,"j":1}"#
    );
    assert_eq!(
        r("{{ o | copyq | json }}", r#"{"o":{"n":null}}"#),
        r#"{"q":null}"#
    );
}

#[test]
fn changing_a_common_object_does_not_change_the_pages_data() {
    let cfg = load(r#"config.add_filter("add", |o| { o.x = 1; o.keys() });"#);
    let e = engine(&cfg);
    let t = e
        .parse("t.liquid", "{{ o | add | json }}{{ o | json }}")
        .unwrap();
    let d = data(r#"{"o":{"z":1}}"#);
    assert_eq!(
        e.render(&t, &d, &NoPartials).unwrap(),
        r#"["z","x"]{"z":1}"#
    );
}

#[test]
fn numbers_are_javascripts() {
    let cfg = load(
        r#"config.add_filter("half", |n| n / 2);
           config.add_filter("sum", |n| n + 0.2);
           config.add_filter("mod", |n, m| n % m);
           config.add_filter("show", |n| `${n}|` + n + "|" + n.to_string());
           config.add_filter("kind", |n| type_of(n));"#,
    );
    let r = |tpl: &str| render(&cfg, tpl, "{}").unwrap();
    assert_eq!(r("{{ 7 | half }}"), "3.5");
    assert_eq!(r("{{ 0.1 | sum }}"), "0.30000000000000004");
    assert_eq!(r("{{ 7 | mod: 3 }}"), "1");
    assert_eq!(r("{{ -7 | mod: 3 }}"), "-1");
    assert_eq!(r("{{ 5.5 | mod: 2 }}"), "1.5");
    assert_eq!(r("{{ 7 | kind }}"), "f64");
    // 2^53 + 1 is 2^53 in JavaScript too; and 5e21 prints as JS prints it.
    assert_eq!(r("{{ 9007199254740993 | half }}"), "4503599627370496");
    assert_eq!(r("{{ 10000000000000000000000 | half }}"), "5e+21");
    // Whole numbers print without `.0`.
    assert_eq!(r("{{ 5 | show }}"), "5|5|5");
    assert_eq!(r("{{ 1.5 | show }}"), "1.5|1.5|1.5");
}

#[test]
fn deep_recursion_in_a_filter_works_on_a_small_stack() {
    let cfg = load(
        "fn depth(n) { if n == 0 { 0 } else { 1 + depth(n - 1) } }\nconfig.add_filter(\"deep\", |v| depth(60));",
    );
    let h = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || render(&cfg, "{{ 1 | deep }}", "{}"))
        .unwrap();
    assert_eq!(h.join().unwrap().unwrap(), "60");
}

#[test]
fn regex_patterns_that_would_match_differently_are_refused() {
    // Each of these compiles in Rust but means something else in
    // JavaScript, so it is an error rather than a silent difference.
    for (pat, flags) in [
        (r"\a", ""),
        (r"\A", ""),
        (r"\p{L}", ""),
        (r"\p{L}", "u"),
        ("[]]", ""),
        ("[^]", ""),
        ("(?i)a", ""),
        ("(?s)a", ""),
        (r"\z", ""),
        (r"\cJ", ""),
        (r"\u{61}", ""),
        ("a{", ""),
        ("a{,2}", ""),
        ("}", ""),
        ("[a--b]", ""),
        (r"[\w-a]", ""),
        ("*a", ""),
        ("^*", ""),
        ("\u{1F600}", ""),
        ("é", "i"),
        ("a", "y"),
        // JavaScript fails an optional repetition that matches nothing.
        ("(a*)?", ""),
        ("(?:b?|a)?", ""),
        ("(?:x|)*", ""),
        ("(\\s*)+", ""),
        // JavaScript clears a group inside a repetition on every pass.
        ("(?:(a)|b)+", "g"),
        ("(?:(a)?b)*", ""),
        ("(?:x(?:(a)|b)){2,}", ""),
    ] {
        let p = load_err(&format!(
            "regex_test(\"a\", {}, {});",
            json(pat),
            json(flags)
        ));
        assert_eq!(p.code, codes::CONFIG_INVALID, "{pat} {flags}: {p:?}");
    }
}

#[test]
fn a_group_set_by_every_repetition_keeps_the_last_value() {
    // Node: "a1b2".replace(/(?:([a-z])\d)+/, "[$1]").
    assert_eq!(
        eval(r#"regex_replace("a1b2", "(?:([a-z])\\d)+", "[$1]", "")"#),
        s("[b]")
    );
}

#[test]
fn regex_text_that_would_match_differently_is_refused() {
    let err = |expr: &str| {
        let p = load_err(&format!("let x = {expr};"));
        assert_eq!(p.code, codes::CONFIG_INVALID, "{expr}: {p:?}");
        p.message
    };
    // The Kelvin sign folds to `k` in Rust's `i`, and not in JavaScript's.
    assert!(err(r#"regex_test("\u212A", "k", "i")"#).contains("`i` flag"));
    // Emoji are two units in JavaScript: `.`, a negated class and an
    // empty match can land between them.
    assert!(err(r#"regex_replace("\U0001F600", "(?:)", "-", "g")"#).contains("outside the BMP"));
    assert!(err(r#"regex_test("\U0001F600", "^.$")"#).contains("outside the BMP"));
    assert!(err(r#"regex_split("a\U0001F600", "\\S")"#).contains("outside the BMP"));
    // Lazy, bounded or back-to-back loops can stop between the halves.
    for pat in [
        r"[^>]+?",
        r"[^a]{1,3}",
        r"[^a]+[^b]+",
        r"[^a]+\\S",
        r"([^a])+",
        r"\\B",
        r"[\\uD7FF-\\uE000]",
    ] {
        let msg = err(&format!(
            r#"regex_replace("\U0001F600", "{pat}", "-", "g")"#
        ));
        assert!(msg.contains("outside the BMP"), "{pat}: {msg}");
    }
    // Greedy loops that something ASCII ends are fine: tag stripping.
    assert_eq!(
        eval(r#"regex_replace("<b>\U0001F600</b>.*\U0001F600", "<[^>]+>|\\.+", "", "g")"#),
        s("\u{1F600}*\u{1F600}")
    );
    // `^` and `$` with `m` also break at \r in JavaScript.
    assert!(err(r#"regex_replace("a\rb", "$", "!", "gm")"#).contains("`m` flag"));
    // The `u` flag makes JavaScript use code points too, so it is allowed.
    assert_eq!(
        eval(r#"regex_replace("\U0001F600", "(?:)", "-", "gu")"#),
        s("-\u{1F600}-")
    );
    // Patterns that can't split a pair are fine with emoji around.
    assert_eq!(
        eval(r#"regex_replace("\U0001F600 a", "\\s+", "_", "g")"#),
        s("\u{1F600}_a")
    );
    // And `m` without `^`/`$`, or without \r, is fine.
    assert_eq!(
        eval(r#"regex_replace("a\r\nb", "b", "!", "gm")"#),
        s("a\r\n!")
    );
    assert_eq!(
        eval(r#"regex_replace("a\nb", "$", "!", "gm")"#),
        s("a!\nb!")
    );
}

fn text(expr: &str) -> String {
    eval(&format!("js_string({expr})")).to_js_string()
}

#[test]
fn json_stringify_is_javascripts() {
    // Node: JSON.stringify(x).
    let j = |expr: &str| eval(&format!("json_stringify({expr})"));
    assert_eq!(j("#{ b: 1 }"), s(r#"{"b":1}"#));
    let o = r#"{
        let o = object();
        o.b = 1;
        o.a = [1, "x", null_value(), (), true, -0.0, 1e21, 0.1 + 0.2, 0.0 / 0.0];
        o.c = "< \"\\\n\u0001é\U0001F600";
        o
    }"#;
    assert_eq!(
        j(o),
        s(
            "{\"b\":1,\"a\":[1,\"x\",null,null,true,0,1e+21,0.30000000000000004,null],\"c\":\"< \\\"\\\\\\n\\u0001\u{e9}\u{1F600}\"}"
        )
    );
    assert_eq!(j(r#""s""#), s(r#""s""#));
    assert_eq!(j("null_value()"), s("null"));
    assert_eq!(j("1.5"), s("1.5"));
    assert_eq!(j("[]"), s("[]"));
    assert_eq!(j("{ let o = object(); o.u = (); o }"), s("{}"));
    assert_eq!(j("()"), Value::Undefined);
    assert_eq!(j("date(0)"), s(r#""1970-01-01T00:00:00.000Z""#));
    // Years outside 0-9999 get six digits and a sign, as in toISOString.
    for (ms, want) in [
        ("-62198755200000", r#""-000001-01-01T00:00:00.000Z""#),
        ("-62167219200001", r#""-000001-12-31T23:59:59.999Z""#),
        ("253402300800000", r#""+010000-01-01T00:00:00.000Z""#),
        ("-8000000000000000", r#""-251540-02-03T09:46:40.000Z""#),
        ("-62135596800000", r#""0001-01-01T00:00:00.000Z""#),
    ] {
        assert_eq!(j(&format!("date({ms})")), s(want), "{ms}");
    }
}

#[test]
fn js_string_is_javascripts_string() {
    // Node: String(x).
    assert_eq!(text("[1, null_value(), (), \"a\", [2, 3]]"), "1,,,a,2,3");
    assert_eq!(text("object()"), "[object Object]");
    assert_eq!(text("#{ a: 1 }"), "[object Object]");
    assert_eq!(text("1e21"), "1e+21");
    assert_eq!(text("0.1 + 0.2"), "0.30000000000000004");
    assert_eq!(text("-0.0"), "0");
    assert_eq!(text("true"), "true");
    assert_eq!(text("null_value()"), "null");
    assert_eq!(text("()"), "undefined");
    assert_eq!(text("[]"), "");
    assert_eq!(text("7"), "7");
}

#[test]
fn js_concat_is_javascripts_plus() {
    // Node: 1+"a", "a"+1, 1+1, undefined+1, null+1, true+1, [1,2]+"x",
    // {}+"x", [1]+[2], 1.5+2, "a"+null, "a"+undefined, null+null,
    // undefined+undefined.
    let c = |a: &str, b: &str| eval(&format!("js_concat({a}, {b})")).to_js_string();
    assert_eq!(c("1", "\"a\""), "1a");
    assert_eq!(c("\"a\"", "1"), "a1");
    assert_eq!(c("1", "1"), "2");
    assert_eq!(c("()", "1"), "NaN");
    assert_eq!(c("null_value()", "1"), "1");
    assert_eq!(c("true", "1"), "2");
    assert_eq!(c("[1, 2]", "\"x\""), "1,2x");
    assert_eq!(c("object()", "\"x\""), "[object Object]x");
    assert_eq!(c("[1]", "[2]"), "12");
    assert_eq!(c("1.5", "2"), "3.5");
    assert_eq!(c("\"a\"", "null_value()"), "anull");
    assert_eq!(c("\"a\"", "()"), "aundefined");
    assert_eq!(c("null_value()", "null_value()"), "0");
    assert_eq!(c("()", "()"), "NaN");
}

fn list(expr: &str) -> String {
    eval(expr).to_js_string()
}

#[test]
fn js_sort_is_the_default_sort() {
    // Node: [3,1,2,undefined,10].sort() is 1,10,2,3 then undefined.
    assert_eq!(list("js_sort([3, 1, 2, (), 10])"), "1,10,2,3,");
    assert_eq!(
        list(
            r#"js_sort([10, 9, 1, "b", (), "a", null_value(), "B", [2], "é", "z", "\U0001F600", "\uFFFF"])"#
        ),
        "1,10,2,9,B,a,b,,z,é,\u{1F600},\u{FFFF},"
    );
}

#[test]
fn js_sort_by_takes_javascript_comparators() {
    assert_eq!(list("js_sort_by([5, 1, 4], |a, b| a - b)"), "1,4,5");
    // A fractional result counts by its sign.
    assert_eq!(
        list("js_sort_by([1.5, 1.2, 1.9, 0.3], |a, b| (a - b) * 0.1)"),
        "0.3,1.2,1.5,1.9"
    );
    assert_eq!(list("js_sort_by([3, 1, 2], |a, b| b > a)"), "3,1,2");
    assert_eq!(
        list(r#"js_sort_by(["b", "a", (), "c"], |a, b| if a < b { -1 } else { 1 })"#),
        "a,b,c,"
    );
    // Stable.
    assert_eq!(
        list(
            r#"js_sort_by([#{ k: 1, n: "a" }, #{ k: 1, n: "b" }, #{ k: 0, n: "c" }], |a, b| a.k - b.k).map(|x| x.n)"#
        ),
        "c,a,b"
    );
    // A comparator that never settles doesn't panic.
    let _ = list("js_sort_by([1, 2, 3, 4, 5, 6, 7, 8, 9, 10], |a, b| 1)");
}

#[test]
fn js_div_and_js_round_are_javascripts() {
    assert_eq!(eval("js_div(7, 2)"), Value::Number(3.5));
    assert_eq!(eval("js_div(1, 0)").to_js_string(), "Infinity");
    assert_eq!(eval("js_div(0, 0)").to_js_string(), "NaN");
    assert_eq!(
        eval("js_div(9007199254740993.0, 2)"),
        Value::Number(4503599627370496.0)
    );
    // Node: Math.round(-4.5, -0.4, 2.5, 0.5, -0.5, -1.5, 0.49999999999999994,
    // 4503599627370497) is -4, -0, 3, 1, -0, -1, 0, 4503599627370497.
    let r = |x: &str| match eval(&format!("js_round({x})")) {
        Value::Number(n) => n,
        other => panic!("{other:?}"),
    };
    assert_eq!(r("-4.5"), -4.0);
    assert!(r("-0.4") == 0.0 && r("-0.4").is_sign_negative());
    assert_eq!(r("2.5"), 3.0);
    assert_eq!(r("0.5"), 1.0);
    assert!(r("-0.5").is_sign_negative() && r("-0.5") == 0.0);
    assert_eq!(r("-1.5"), -1.0);
    assert_eq!(r("0.49999999999999994"), 0.0);
    assert_eq!(r("4503599627370497.0"), 4503599627370497.0);
    assert_eq!(r("3"), 3.0);
}

#[test]
fn text_made_from_brrrty_types_is_javascripts_never_a_dump() {
    let cfg = load(
        r#"config.add_filter("a", |o| `x${o}y`);
           config.add_filter("b", |o| "p" + o);
           config.add_filter("c", |o| o + "q");
           config.add_filter("d", |n| `${n}`);
           config.add_filter("e", |o| [o, o.n].to_string());
           config.add_filter("f", |o| o.to_debug());"#,
    );
    let r = |tpl: &str| render(&cfg, tpl, r#"{"o":{"n":null},"n":null}"#).unwrap();
    assert_eq!(r("{{ o | a }}"), "x[object Object]y");
    assert_eq!(r("{{ o | b }}"), "p[object Object]");
    assert_eq!(r("{{ o | c }}"), "[object Object]q");
    assert_eq!(r("{{ n | d }}"), "null");
    assert!(
        !r("{{ o | e }}").contains("JsObject"),
        "{}",
        r("{{ o | e }}")
    );
    assert_eq!(r("{{ o | f }}"), "[object Object]");
}

#[test]
fn the_i_flag_only_matters_when_the_pattern_has_letters() {
    // No letters to fold, so non-ASCII text is fine (tag stripping with `gi`).
    assert_eq!(
        eval(r#"regex_replace("\u00e9\u2014<b>x</b>", "(<([^>]+)>)", "", "gi")"#),
        s("\u{e9}\u{2014}x")
    );
    for pat in ["a", "[a-z]", "\\w", "\\x41"] {
        let p = load_err(&format!("regex_test(\"\\u00e9\", {}, \"i\");", json(pat)));
        assert!(p.message.contains("`i` flag"), "{pat}: {}", p.message);
    }
}

// ---- no captures: nothing carries state from one call to the next ----

#[test]
fn a_closure_that_captures_a_variable_is_refused_at_load_with_its_line() {
    for script in [
        "let n = 0;\nconfig.add_filter(\"f\", |v| { n += 1; n });",
        "let c = object();\nconfig.add_filter(\"f\", |v| { c.n = 1; c.n });",
        "let bump = || 1;\nconfig.add_filter(\"f\", |v| bump.call());",
        "let t = #{ a: 1 };\nconfig.add_filter(\"f\", |v| t[v]);",
        "let d = now();\nconfig.add_filter(\"f\", |v| { d = now(); 1 });",
        "let n = 0;\nfn f(v) { n }\nconfig.add_filter(\"f\", Fn(\"f\"));",
    ] {
        let p = load_err(script);
        assert_eq!(p.code, codes::CONFIG_SYNTAX, "{script}: {p:?}");
        assert!(p.line.is_some_and(|l| l >= 2), "{script}: {p:?}");
        assert!(
            p.help.as_deref().unwrap_or_default().contains("PORTING.md"),
            "{script}: {p:?}"
        );
    }
}

#[test]
fn a_curried_value_can_be_read() {
    let cfg = load(
        r#"
        let table = object();
        table.x = 10;
        let rates = #{ a: 2, b: 3 };
        fn pick(t, v) { t[v] }
        fn rate(r, v, k) { r[k] * v }
        config.add_filter("pick", Fn("pick").curry(table));
        config.add_filter("rate", Fn("rate").curry(rates));
        "#,
    );
    let r = |tpl: &str| render(&cfg, tpl, "{}").unwrap();
    assert_eq!(r("{{ 'x' | pick }}"), "10");
    // The curried value comes first, then what the template passes.
    assert_eq!(r("{{ 5 | rate: 'b' }}"), "15");
}

/// A site of `n` pages, each `{{ 1 | f }}-{{ data | json }}`, and what each
/// page rendered.
fn many_pages(script: &str, n: usize, tpl: &str) -> Vec<String> {
    let pages: Vec<(String, String)> = (0..n)
        .map(|i| (format!("p{i:02}.liquid"), tpl.to_string()))
        .collect();
    let refs: Vec<(&str, &str)> = pages
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let (t, r) = site(script, &refs);
    assert!(!r.has_errors(), "{:?}", r.problems);
    (0..n)
        .map(|i| fs::read_to_string(t.path().join(format!("_site/p{i:02}/index.html"))).unwrap())
        .collect()
}

#[test]
fn a_filter_that_changes_its_argument_does_not_change_other_pages() {
    // Global data is common by every page. The filter changes what it is
    // given; every page must still see the original.
    let out = many_pages(
        r#"
        let common = object();
        common.hits = 0;
        common.list = [object()];
        config.add_global_data("common", common);
        config.add_filter("bump", |o| {
            o.hits = o.hits + 1;
            o.list.push(1);
            o.list[0].x = 1;
            o.hits
        });
        "#,
        50,
        "{{ common | bump }}-{{ common | json }}",
    );
    assert_eq!(out.len(), 50);
    for (i, page) in out.iter().enumerate() {
        assert_eq!(page, r#"1-{"hits":0,"list":[{}]}"#, "page {i}");
    }
}

#[test]
fn a_curried_value_is_fresh_for_every_call() {
    let out = many_pages(
        r#"
        let c = object();
        c.n = 0;
        let list = [1];
        fn bump(c, v) { c.n += 1; c.n }
        fn grow(list, v) { list.push(1); list.len() }
        config.add_filter("bump", Fn("bump").curry(c));
        config.add_filter("grow", Fn("grow").curry(list));
        "#,
        50,
        "{{ 1 | bump }}{{ 1 | grow }}",
    );
    assert!(out.iter().all(|p| p == "12"), "{out:?}");
}

#[test]
fn a_function_has_no_hash_in_its_text() {
    let cfg = load(
        r#"config.add_filter("f", |v| { let g = |x| x; `${g}|` + g + "|" + g.to_string() });"#,
    );
    let want = "function () { [native code] }";
    assert_eq!(
        render(&cfg, "{{ 1 | f }}", "{}").unwrap(),
        format!("{want}|{want}|{want}")
    );
}

/// A comparator that throws in a filter's `sort` refuses the page; a
/// working one sorts as JavaScript does.
#[test]
fn a_filter_sort_with_a_failing_comparator_refuses_its_page() {
    let (t, r) = site(
        "config.add_filter(\"bad\", |v| v.sort(|a, b| { throw \"cmp broke\" }));\nconfig.add_filter(\"good\", |v| v.sort(|a, b| if a < b { -1 } else if a > b { 1 } else { 0 }));\n",
        &[
            (
                "good.liquid",
                "{{ '3,1,2' | split: ',' | good | join: ',' }}",
            ),
            ("bad.liquid", "{{ '3,1,2' | split: ',' | bad | join: ',' }}"),
        ],
    );
    assert_eq!(
        fs::read_to_string(t.path().join("_site/good/index.html")).unwrap(),
        "1,2,3"
    );
    assert!(!t.path().join("_site/bad/index.html").exists());
    assert!(
        r.problems.iter().any(|p| p.message.contains("cmp broke")),
        "{:#?}",
        r.problems
    );
}

/// A filter that calls Rhai's `timestamp()` (the real clock) refuses its page instead of
/// writing a different value on every build. `tests/config.rs` checks the top level.
#[test]
fn a_filter_calling_timestamp_is_refused() {
    let cfg = load("config.add_filter(\"el\", |v| timestamp().elapsed.to_string());");
    let why = render(&cfg, "{{ 1 | el }}", "{}").unwrap_err();
    assert!(why.contains("timestamp() is not available"), "{why}");
    assert!(why.contains("Use now()"), "{why}");
}

/// A filter that fills an `object()` past Brrrty's size limit is stopped with Rhai's
/// data-too-large error, and the next call starts its count afresh.
#[test]
fn a_filter_filling_an_object_is_stopped_by_size() {
    let cfg = load(
        "config.add_filter(\"fill\", |n| { let big = \"x\"; for k in 0..23 { big += big; } \
         let o = object(); for i in 0..n.to_int() { o[i.to_string()] = big + i; } o.len() });",
    );
    let why = render(&cfg, "{{ 20 | fill }}", "{}").unwrap_err();
    assert!(why.contains("Total size of objects (over 64 MB"), "{why}");
    assert_eq!(render(&cfg, "{{ 7 | fill }}", "{}").unwrap(), "7");
    assert_eq!(render(&cfg, "{{ 7 | fill }}", "{}").unwrap(), "7");
}

/// 11ty gives a JavaScript filter LiquidJS's drop objects for `nil`, `null`, `empty`, `blank`
/// and `forloop`, assigned or not. Node printed, for a filter echoing its input:
/// `nil` and `null` are a NullDrop (`typeof` "object", `JSON.stringify` `{}`, `String`
/// `[object Object]`, `v + ""` "null", truthy), `empty` and `blank` the same but `v + ""` is
/// "", and `forloop` stringifies as `{"i":0,"length":1,"name":"i-(1..1)"}`. No value a script
/// can get acts like that, so each refuses the page. `null` from data, and an undefined
/// variable, are plain values and pass.
#[test]
fn liquid_drops_given_to_a_filter_refuse_the_page() {
    let cfg =
        load("config.add_filter(\"f\", |v, a| json_stringify(v) + \":\" + json_stringify(a));");
    for (tpl, drop, what) in [
        ("{{ nil | f }}", "NullDrop", "its input"),
        ("{{ null | f }}", "NullDrop", "its input"),
        ("{{ empty | f }}", "EmptyDrop", "its input"),
        ("{{ blank | f }}", "BlankDrop", "its input"),
        ("{{ 1 | f: nil }}", "NullDrop", "an argument"),
        ("{{ 1 | f: empty }}", "EmptyDrop", "an argument"),
        ("{{ 1 | f: k: blank }}", "BlankDrop", "its `k` argument"),
        ("{% assign a = nil %}{{ a | f }}", "NullDrop", "its input"),
        (
            "{% assign b = empty %}{{ 1 | f: b }}",
            "EmptyDrop",
            "an argument",
        ),
        (
            "{% for i in (1..1) %}{{ forloop | f }}{% endfor %}",
            "ForloopDrop",
            "its input",
        ),
    ] {
        let why = render(&cfg, tpl, "{}").unwrap_err();
        assert!(
            why.contains(&format!("LiquidJS's {drop}, as {what}")),
            "{tpl}: {why}"
        );
    }
    assert_eq!(
        render(&cfg, "{{ x | f: y }}|{{ n | f: n }}", "{\"n\": null}").unwrap(),
        ":|null:null"
    );
}

// ---- the same filters in the Nunjucks engine ----

/// A Rhai filter registered in a Nunjucks engine, rendering `tpl`.
fn render_njk(cfg: &SiteConfig, tpl: &str, json: &str) -> Result<String, String> {
    use brrrty_nunjucks::{Engine, EngineOptions};
    struct NoIncludes;
    impl brrrty_nunjucks::Loader for NoIncludes {
        fn load(&self, _: &str) -> Result<(String, String), brrrty_nunjucks::LoadError> {
            Err(Default::default())
        }
    }
    let mut e = Engine::new(EngineOptions {
        timezone: brrrty_nunjucks::Tz::Utc,
        autoescape: true,
    });
    brrrty_core::register_nunjucks_filters(&mut e, &cfg.filters, false);
    let t = e.compile("t.njk", tpl).map_err(|e| e.to_string())?;
    e.render(&t, &data(json), &NoIncludes)
        .map_err(|e| e.to_string())
}

#[test]
fn a_rhai_filter_runs_in_nunjucks_with_the_arguments_as_written() {
    let cfg = load(r#"config.add_filter("f", |v, a, b, c| `${v}|${a}|${b}|${c}`);"#);
    assert_eq!(
        render_njk(&cfg, "{{ s | f(1, 'x') }}", r#"{"s": "in"}"#).unwrap(),
        "in|1|x|"
    );
    // Nunjucks passes keyword arguments as a last object, as 11ty does to
    // a JavaScript filter, so the keyword arrives where a positional would.
    assert_eq!(
        render_njk(&cfg, "{{ s | f(k=1) }}", r#"{"s": "in"}"#).unwrap(),
        "in|[object Object]||"
    );
}

#[test]
fn a_rhai_filter_replaces_a_nunjucks_one_and_refuses_a_safe_string() {
    let cfg = load(r#"config.add_filter("upper", |v| `[${v}]`);"#);
    assert_eq!(
        render_njk(&cfg, "{{ s | upper }}", r#"{"s": "a"}"#).unwrap(),
        "[a]"
    );
    let err = render_njk(&cfg, "{{ (s | safe) | upper }}", r#"{"s": "a"}"#).unwrap_err();
    assert!(err.contains("SafeString"), "{err}");
}

fn string(expr: &str) -> String {
    match eval(expr) {
        Value::String(s) => s.to_string(),
        other => panic!("{expr}: not a string: {other:?}"),
    }
}

#[test]
fn to_fixed_is_v8s() {
    // Node: (2.5).toFixed(0), (1.005).toFixed(2), (-0).toFixed(2),
    // (-0.0001).toFixed(2), (1e21).toFixed(2), (1234.5678).toFixed(1), (7).toFixed(3)
    assert_eq!(string("to_fixed(2.5, 0)"), "3");
    assert_eq!(string("to_fixed(1.005, 2)"), "1.00");
    assert_eq!(string("to_fixed(-0.0, 2)"), "0.00");
    assert_eq!(string("(-0.0001).to_fixed(2)"), "-0.00");
    assert_eq!(string("to_fixed(1e21, 2)"), "1e+21");
    assert_eq!(string("1234.5678.to_fixed(1)"), "1234.6");
    assert_eq!(string("to_fixed(7, 3)"), "7.000");
    // Digits are truncated, as ToIntegerOrInfinity does.
    assert_eq!(string("to_fixed(1.25, 1.9)"), "1.3");
    let p = load_err("to_fixed(1.0, 101)");
    assert!(p.message.contains("between 0 and 100"), "{}", p.message);
    let p = load_err(r#"to_fixed("5", 1)"#);
    assert!(p.message.contains("to_fixed"), "{}", p.message);
}

#[test]
fn to_locale_string_is_en_us() {
    // Node: (1234.5678).toLocaleString('en-US'), (-0.0004)..., (1e21)...,
    // (1234567).toLocaleString(), (0.0005)...
    assert_eq!(string("to_locale_string(1234.5678)"), "1,234.568");
    assert_eq!(string(r#"to_locale_string(-0.0004, "en-US")"#), "-0");
    assert_eq!(
        string("to_locale_string(1e21)"),
        "1,000,000,000,000,000,000,000"
    );
    assert_eq!(string("1234567.to_locale_string()"), "1,234,567");
    assert_eq!(string(r#"0.0005.to_locale_string("en")"#), "0.001");
    let p = load_err(r#"to_locale_string(1234.5, "de-DE")"#);
    assert!(p.message.contains("de-DE"), "{}", p.message);
    let p = load_err(r#"to_locale_string(1234.5, "en-US", #{ maximumFractionDigits: 1 })"#);
    assert!(p.message.contains("options"), "{}", p.message);
}

#[test]
fn locale_compare_is_icus_root_order() {
    // Node (en-US): "a".localeCompare("B"), "résumé" vs "resume",
    // "co-op" vs "coop", "10" vs "9", "a" vs "a"
    assert_eq!(eval(r#"locale_compare("a", "B")"#), Value::Number(-1.0));
    assert_eq!(
        eval(r#""résumé".locale_compare("resume")"#),
        Value::Number(1.0)
    );
    assert_eq!(
        eval(r#"locale_compare("co-op", "coop")"#),
        Value::Number(-1.0)
    );
    assert_eq!(
        eval(r#"locale_compare("10", "9", "en-US")"#),
        Value::Number(-1.0)
    );
    assert_eq!(eval(r#"locale_compare("a", "a")"#), Value::Number(0.0));
    assert_eq!(
        list(r#"["b", "B", "a", "é", "A", "e"].sort(|x, y| locale_compare(x, y))"#),
        "a,A,b,B,e,é"
    );
    let p = load_err(r#"locale_compare("a", "b", "sv-SE")"#);
    assert!(p.message.contains("sv-SE"), "{}", p.message);
    let p = load_err(r#"locale_compare("a\u20C2", "b")"#);
    assert!(p.message.contains("U+20C2"), "{}", p.message);
    // Node sorts Han by radical and stroke ("﨑" before "嵊"); refused.
    let p = load_err(r#"locale_compare("﨑", "嵊")"#);
    assert!(p.message.contains("Han character"), "{}", p.message);
}

#[test]
fn random_is_refused_with_the_reason() {
    let p = load_err("random()");
    assert!(p.message.contains("Math.random"), "{}", p.message);
    assert!(p.message.contains("add_random_filter"), "{}", p.message);
}

#[test]
fn local_dates_from_fields() {
    // The runner's zone is whatever TZ says; these don't depend on it.
    let r = |script: &str| load(script).global_data["r"].to_js_string();
    assert_eq!(
        r("let d = date(2026, 4, 10);
           config.add_global_data(\"r\", `${d.get_full_year()}-${d.get_month()}-${d.get_date()}-${d.get_day()}`);"),
        "2026-4-10-0"
    );
    // Month 12 is next January, day 0 the day before, hour 25 the next day.
    assert_eq!(
        r("let d = date(2026, 12, 0, 25, 61, 1, 5);
           config.add_global_data(\"r\", [d.get_full_year(), d.get_month(), d.get_date(), d.get_hours(),
               d.get_minutes(), d.get_seconds(), d.get_milliseconds()]);"),
        "2027,0,1,2,1,1,5"
    );
    assert_eq!(string("date(99, 0).get_full_year().to_string()"), "1999");
    assert_eq!(
        string("date(0).to_iso_string()"),
        "1970-01-01T00:00:00.000Z"
    );
    assert_eq!(
        string("date(-62198755200000).to_iso_string()"),
        "-000001-01-01T00:00:00.000Z"
    );
    let p = load_err("date(()).to_iso_string()");
    assert!(p.message.contains("Invalid time value"), "{}", p.message);
    assert!(matches!(
        eval("date(2026, 0, 1).get_timezone_offset()"),
        Value::Number(_)
    ));
}
