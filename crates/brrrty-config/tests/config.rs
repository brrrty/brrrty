//! `brrrty.config.rhai`: what a script sets, and how a bad script is stopped.
//! The 11ty behavior these mirror was probed with 11ty 3.1.8; the fixture
//! `tests/fixtures/config` pins a whole build against real 11ty output.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use brrrty_config::{CONFIG_FILE, Clock, PassthroughEntry, SiteConfig, load_with};
use brrrty_core::{Problem, Severity, codes};
use brrrty_js::Value;

/// 2023-11-14T22:13:20Z.
const EPOCH_MS: i64 = 1_700_000_000_000;

fn run(script: &str) -> Result<Option<SiteConfig>, Vec<Problem>> {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join(CONFIG_FILE), script).unwrap();
    load_with(t.path(), Clock::Fixed(EPOCH_MS))
}

fn ok(script: &str) -> SiteConfig {
    match run(script) {
        Ok(Some(c)) => c,
        Ok(None) => panic!("no config"),
        Err(p) => panic!("config failed: {p:#?}"),
    }
}

fn err(script: &str) -> Problem {
    let started = Instant::now();
    let r = run(script);
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "took {:?}",
        started.elapsed()
    );
    match r {
        Err(mut p) => {
            assert_eq!(p.len(), 1, "{p:#?}");
            let p = p.remove(0);
            assert_eq!(p.severity, Severity::Error);
            assert_eq!(p.path.as_deref(), Some(Path::new(CONFIG_FILE)));
            p
        }
        Ok(c) => panic!("expected a problem, got {:?}", c.map(|c| c.dir)),
    }
}

fn s(v: &str) -> Value {
    Value::String(v.into())
}

#[test]
fn no_config_file_is_none() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("eleventy.config.js"), "x").unwrap();
    assert!(load_with(t.path(), Clock::System).unwrap().is_none());
}

#[test]
fn an_empty_script_is_the_defaults() {
    let c = ok("");
    assert_eq!(c.dir.input, None);
    assert_eq!(c.dir.output, None);
    assert!(c.passthrough.is_empty() && c.ignores.is_empty() && c.global_data.is_empty());
    assert_eq!(c.file, Path::new(CONFIG_FILE));
}

#[test]
fn the_last_expression_is_the_returned_object() {
    let c = ok(r#"#{ dir: #{ input: "src", output: "dist" } }"#);
    assert_eq!(c.dir.input.as_deref(), Some("src"));
    assert_eq!(c.dir.output.as_deref(), Some("dist"));
    // The default folder names are fine, spelled any way.
    ok(r#"#{ dir: #{ includes: "_includes", data: "./_data/", layouts: "_includes" } }"#);
}

/// 11ty's `markdownTemplateEngine` and `htmlTemplateEngine`: a language
/// or `false`. `dataTemplateEngine` is warned about: 11ty 3 ignores it
/// (probed: a data file's `{{ }}` is printed as written).
#[test]
fn template_engines_are_read_from_the_returned_object() {
    use brrrty_core::{TemplateEngines, TemplateLanguage};
    assert_eq!(ok("").template_engines, TemplateEngines::default());
    let c = ok(r#"#{ markdownTemplateEngine: "njk", htmlTemplateEngine: false }"#);
    assert_eq!(
        c.template_engines,
        TemplateEngines {
            markdown: Some(TemplateLanguage::Nunjucks),
            html: None,
        }
    );
    assert_eq!(c.build_options().template_engines, c.template_engines);
    let c = ok(r#"#{ dataTemplateEngine: "njk" }"#);
    assert_eq!(c.problems.len(), 1, "{:?}", c.problems);
    assert!(c.problems[0].message.contains("dataTemplateEngine"));
    let p = err(r#"#{ htmlTemplateEngine: "ejs" }"#);
    assert_eq!(p.code, codes::CONFIG_UNSUPPORTED);
    let p = err(r#"#{ markdownTemplateEngine: 3 }"#);
    assert_eq!(p.code, codes::CONFIG_INVALID);
}

#[test]
fn unsupported_returned_keys_are_refused() {
    let p = err(r#"#{ dir: #{ input: "src" }, pathPrefix: "/x/" }"#);
    assert_eq!(p.code, codes::CONFIG_UNSUPPORTED);
    assert!(p.message.contains("pathPrefix"), "{}", p.message);
    let p = err(r#"#{ dir: #{ input: 3 } }"#);
    assert_eq!(p.code, codes::CONFIG_INVALID);
    let p = err(r#""src""#);
    assert_eq!(p.code, codes::CONFIG_INVALID);
}

/// `dir.includes` and `dir.layouts` are folders inside the input folder.
#[test]
fn includes_and_layouts_folders_are_read() {
    let c = ok(r#"#{ dir: #{ includes: "./parts/", layouts: "parts/layouts" } }"#);
    assert_eq!(c.dir.includes.as_deref(), Some("parts"));
    assert_eq!(c.dir.layouts.as_deref(), Some("parts/layouts"));
    let o = c.build_options();
    assert_eq!(o.includes_dir(), "parts");
    assert_eq!(o.layouts_dir(), "parts/layouts");
    let c = ok(r#"#{ dir: #{ includes: "parts" } }"#);
    assert_eq!(c.build_options().layouts_dir(), "parts");
    assert_eq!(ok("").build_options().layouts_dir(), "_includes");
    for (v, part) in [
        ("../shared", "inside the input folder"),
        ("/abs", "inside the input folder"),
        ("", "inside the input folder"),
        (".", "inside the input folder"),
        ("a/../b", "inside the input folder"),
        ("a\\\\b", "inside the input folder"),
    ] {
        let p = err(&format!("#{{ dir: #{{ layouts: \"{v}\" }} }}"));
        assert_eq!(p.code, codes::CONFIG_UNSUPPORTED, "{v}");
        assert!(p.message.contains(part), "{v}: {}", p.message);
    }
}

/// 11ty's `addLayoutAlias`: a later alias of the same name wins.
#[test]
fn layout_aliases_are_kept() {
    let c = ok(r#"
        config.add_layout_alias("post", "layouts/post.njk");
        config.add_layout_alias("page", "page.liquid");
        config.add_layout_alias("post", "layouts/article.njk");
    "#);
    let o = c.build_options();
    assert_eq!(
        o.layout_aliases.iter().collect::<Vec<_>>(),
        [
            (&"post".to_string(), &"layouts/article.njk".to_string()),
            (&"page".to_string(), &"page.liquid".to_string())
        ]
    );
}

#[test]
fn passthrough_keeps_11ty_order_and_replaces_by_source() {
    // 11ty keeps passthrough copies in an object keyed by the source as
    // written: a later call for the same source replaces its target in place.
    let c = ok(r#"
        config.add_passthrough_copy("src/images/");
        config.add_passthrough_copy(#{ "src/favicon.svg": "favicon.svg" });
        config.add_passthrough_copy("top");
        config.add_passthrough_copy(#{ "src/favicon.svg": "/icons/fav.svg" });
        config.add_passthrough_copy(#{ "top": "toplevel" });
    "#);
    let e = |from: &str, to: Option<&str>| PassthroughEntry {
        from: from.into(),
        to: to.map(Into::into),
    };
    assert_eq!(
        c.passthrough,
        vec![
            e("src/images/", None),
            e("src/favicon.svg", Some("/icons/fav.svg")),
            e("top", Some("toplevel")),
        ]
    );
}

#[test]
fn ignores_are_kept_in_order() {
    let c = ok(r#"
        config.ignores.add("src/games/**/*.md");
        config.ignores.add("drafts");
        config.ignores.add("drafts");
    "#);
    assert_eq!(c.ignores, vec!["src/games/**/*.md", "drafts"]);
}

#[test]
fn global_data_from_now_uses_the_fixed_clock() {
    let c = ok(r#"
        let d = now();
        config.add_global_data("build", #{
            date: date_to_utc_string(d),
            year: year(d),
            ms: get_time(d),
            when: d,
        });
        config.add_global_data("list", [1, 2.5, true, (), "x"]);
    "#);
    let Value::Object(build) = &c.global_data["build"] else {
        panic!("{:?}", c.global_data);
    };
    assert_eq!(build["date"], s("Tue, 14 Nov 2023 22:13:20 GMT"));
    // `getFullYear` is local time; the instant is the same year everywhere.
    assert_eq!(build["year"], Value::Number(2023.0));
    assert_eq!(build["ms"], Value::Number(EPOCH_MS as f64));
    let Value::Date(d) = &build["when"] else {
        panic!("{:?}", build["when"]);
    };
    assert_eq!(d.timestamp_millis(), EPOCH_MS);
    assert_eq!(
        c.global_data["list"],
        Value::Array(
            vec![
                Value::Number(1.0),
                Value::Number(2.5),
                Value::Bool(true),
                Value::Undefined,
                s("x")
            ]
            .into()
        )
    );
}

#[test]
fn global_data_merges_like_add_global_data() {
    // UserConfig.addGlobalData: two plain objects merge deeply, anything
    // else replaces; a function is called for its value.
    let c = ok(r#"
        config.add_global_data("site", #{ a: 1, nested: #{ x: 1 } });
        config.add_global_data("other", 1);
        config.add_global_data("site", #{ b: 2, nested: #{ y: 2 } });
        config.add_global_data("other", "two");
        config.add_global_data("made", || #{ made: get_time(now()) });
        config.add_global_data("a.b", "dotted");
    "#);
    let keys: Vec<&str> = c.global_data.keys().map(String::as_str).collect();
    assert_eq!(keys, ["site", "other", "made", "a.b"]);
    let Value::Object(site) = &c.global_data["site"] else {
        panic!()
    };
    assert_eq!(site["a"], Value::Number(1.0));
    assert_eq!(site["b"], Value::Number(2.0));
    let Value::Object(nested) = &site["nested"] else {
        panic!()
    };
    assert_eq!(nested.len(), 2);
    assert_eq!(c.global_data["other"], s("two"));
    let Value::Object(made) = &c.global_data["made"] else {
        panic!()
    };
    assert_eq!(made["made"], Value::Number(EPOCH_MS as f64));
    assert_eq!(c.global_data["a.b"], s("dotted"));
}

#[test]
fn values_that_cannot_be_data_are_refused_where_they_were_added() {
    let p = err("\n\nconfig.add_global_data(\"x\", #{ f: || 1 });");
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert_eq!(p.line, Some(3));
}

#[test]
fn a_syntax_error_has_its_line_and_column() {
    let p = err("let a = 1;\nlet b = ;\n");
    assert_eq!(p.code, codes::CONFIG_SYNTAX);
    assert_eq!((p.line, p.column), (Some(2), Some(9)));
    assert!(
        !p.message.contains("line 2"),
        "position is not repeated: {}",
        p.message
    );
}

#[test]
fn a_runtime_error_has_its_line_and_column() {
    let p = err("let a = 1;\n  config.add_widget(\"x\", || 1);\n");
    assert_eq!(p.code, codes::CONFIG_ERROR);
    assert_eq!(p.line, Some(2));
    assert!(p.message.contains("add_widget"), "{}", p.message);
    let p = err(r#"throw "custom";"#);
    assert_eq!(p.code, codes::CONFIG_ERROR);
    assert!(p.message.contains("custom"), "{}", p.message);
}

#[test]
fn a_billion_iterations_are_stopped() {
    let p = err("let n = 0;\nfor i in 0..1000000000 { n += 1; }\n");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
    assert_eq!(p.line, Some(2));
    let p = err("loop {}");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
}

#[test]
fn work_that_operations_do_not_count_is_stopped_by_time() {
    // Each `[a]` copies all of `a`, so this is quadratic in one operation
    // per pass; the clock stops it.
    let p = err("let a = [];\nfor i in 0..90000 { a = [a]; }\n");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
    assert!(p.message.contains("seconds"), "{}", p.message);
}

#[test]
fn deep_recursion_is_stopped() {
    let p = err("fn f(n) { f(n + 1) }\nf(0)\n");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
    assert!(
        p.message.len() < 400,
        "one message, not one per call: {}",
        p.message
    );
    let p = err("let f = |n| n;\nfn g(x) { g(x) + 1 }\ng(1)");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
}

#[test]
fn a_huge_string_is_stopped() {
    let p = err("let s = \"x\";\nloop { s += s; }\n");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
    let p = err("let a = [];\nloop { a.push(a.len()); }\n");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
    let p = err("let m = #{};\nlet i = 0;\nloop { m[`k${i}`] = i; i += 1; }\n");
    assert_eq!(p.code, codes::CONFIG_LIMIT);
}

/// Rhai can't see inside an `object()`, so Brrrty counts what objects hold. Filling one with
/// 8 MB strings used to grow past 7 GB before the clock stopped it; now it stops at 64 MB,
/// whether the strings go in one object, in objects inside it, or in objects in an array.
#[test]
fn filling_objects_is_stopped_by_size() {
    let big = "let big = \"x\";\nfor n in 0..23 { big += big; }\nlet i = 0;\n";
    for fill in [
        "let o = object();\nloop { o[i.to_string()] = big + i; i += 1; }\n",
        "let o = object();\nloop { let p = object(); p.s = big + i; o[`k${i}`] = p; i += 1; }\n",
        "let a = [];\nloop { let p = object(); p.s = [big + i]; a.push(p); i += 1; }\n",
        "let o = object();\nloop { o[`k${i}`] = #{ s: big + i }; i += 1; }\n",
    ] {
        let started = Instant::now();
        let p = err(&format!("{big}{fill}"));
        assert_eq!(p.code, codes::CONFIG_LIMIT, "{fill}");
        assert_eq!(p.line, Some(5), "{fill}");
        assert!(
            p.message.contains("Total size of objects (over 64 MB"),
            "{}",
            p.message
        );
        assert!(started.elapsed() < Duration::from_secs(5), "{fill}");
    }
    // Replacing a value, or removing one, gives its size back.
    let c = ok(&format!(
        "{big}let o = object();\nfor n in 0..20 {{ o.s = big + n; o.t = big + n; o.remove(\"t\"); }}\nconfig.add_global_data(\"n\", o.s.len());\n"
    ));
    assert_eq!(c.global_data["n"], Value::Number(8_388_610.0));
}

#[test]
fn deeply_nested_data_is_stopped_without_overflowing() {
    let p = err("let a = [];\nfor i in 0..2000 { a = [a]; }\nconfig.add_global_data(\"a\", a);\n");
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(p.message.contains("deeper"), "{}", p.message);
    assert_eq!(p.line, Some(3));
}

/// Rhai's `timestamp()` reads the real clock, so it is refused with a pointer to `now()`, at
/// the top level. `tests/filters.rs` checks a filter that calls it.
#[test]
fn timestamp_is_refused_and_points_to_now() {
    for script in ["timestamp()", "let t = timestamp();\nt.elapsed"] {
        let p = err(script);
        assert_eq!(p.code, codes::CONFIG_INVALID);
        assert_eq!(p.line, Some(1));
        assert!(p.message.contains("Use now()"), "{}", p.message);
    }
}

#[test]
fn scripts_cannot_import_or_eval() {
    assert_eq!(err(r#"import "x" as x;"#).code, codes::CONFIG_ERROR);
    assert_eq!(err(r#"eval("1")"#).code, codes::CONFIG_SYNTAX);
}

#[test]
fn source_date_epoch_must_be_whole_seconds() {
    assert_eq!(Clock::parse(None).unwrap(), Clock::System);
    assert_eq!(
        Clock::parse(Some("1700000000")).unwrap(),
        Clock::Fixed(EPOCH_MS)
    );
    for bad in ["", "1.5", "-1", "x", "99999999999999999999"] {
        let p = Clock::parse(Some(bad)).unwrap_err();
        assert_eq!(p.code, codes::SOURCE_DATE_EPOCH_INVALID, "{bad}");
    }
}

#[test]
fn utc_strings_match_javascript() {
    // Probed with Node: `new Date(t).toUTCString()`.
    let c = ok(r#"
        config.add_global_data("a", date_to_utc_string(date(0)));
        config.add_global_data("b", date_to_utc_string(date(-62198755200000)));
        config.add_global_data("c", date_to_utc_string(date(253402300800000)));
        config.add_global_data("d", date_to_utc_string(date(1000000000123)));
        config.add_global_data("e", date_to_utc_string(date(8.64e15 + 1.0)));
    "#);
    let g = |k: &str| c.global_data[k].clone();
    assert_eq!(g("a"), s("Thu, 01 Jan 1970 00:00:00 GMT"));
    assert_eq!(g("b"), s("Fri, 01 Jan -0001 00:00:00 GMT"));
    assert_eq!(g("c"), s("Sat, 01 Jan 10000 00:00:00 GMT"));
    assert_eq!(g("d"), s("Sun, 09 Sep 2001 01:46:40 GMT"));
    assert_eq!(g("e"), s("Invalid Date"));
}

#[test]
fn without_source_date_epoch_now_is_the_clock() {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join(CONFIG_FILE),
        r#"config.add_global_data("ms", get_time(now()));"#,
    )
    .unwrap();
    let c = load_with(t.path(), Clock::System).unwrap().unwrap();
    let Value::Number(ms) = c.global_data["ms"] else {
        panic!("{:?}", c.global_data)
    };
    let real = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    assert!((real - ms).abs() < 60_000.0, "{ms} vs {real}");
}

#[test]
fn a_config_path_that_cannot_be_read_is_unreadable() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join(CONFIG_FILE)).unwrap();
    let mut p = load_with(t.path(), Clock::Fixed(EPOCH_MS)).unwrap_err();
    assert_eq!(p.len(), 1, "{p:#?}");
    let p = p.remove(0);
    assert_eq!(p.code, codes::CONFIG_UNREADABLE);
    assert_eq!(p.path.as_deref(), Some(Path::new(CONFIG_FILE)));
    assert!(p.message.starts_with("cannot read"), "{}", p.message);
}

#[test]
fn dir_must_be_a_map_and_unknown_folders_are_warned_about() {
    let p = err(r#"#{ dir: "src" }"#);
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(p.message.contains("map of folders"), "{}", p.message);

    let c = ok(r#"#{ dir: #{ input: "src", assets: "static" } }"#);
    assert_eq!(c.dir.input.as_deref(), Some("src"));
    assert_eq!(c.problems.len(), 1, "{:#?}", c.problems);
    let w = &c.problems[0];
    assert_eq!(w.severity, Severity::Warning);
    assert_eq!(w.code, codes::CONFIG_UNSUPPORTED);
    assert!(w.message.contains("dir.assets"), "{}", w.message);
}

#[test]
fn a_passthrough_target_must_be_a_string() {
    let p = err("\nconfig.add_passthrough_copy(#{ \"src/a\": 3 });");
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert_eq!(p.line, Some(2));
    assert!(p.message.contains("`src/a`"), "{}", p.message);
    assert!(p.message.contains("must be a string"), "{}", p.message);
}

#[test]
fn ignores_can_be_deleted() {
    let c = ok(r#"
        config.ignores.add("a");
        config.ignores.add("b");
        config.ignores.delete("b");
        config.ignores.delete("never-added");
        config.ignores.add("c");
    "#);
    assert_eq!(c.ignores, vec!["a", "c"]);
}

/// A global data function runs after the script: what it throws, and what it returns that
/// can't be data, are reported at the call that added it.
#[test]
fn global_data_functions_that_fail_are_reported() {
    let p = err("\n\nconfig.add_global_data(\"x\", || throw \"no data today\");");
    assert_eq!(p.code, codes::CONFIG_ERROR);
    assert!(p.message.contains("no data today"), "{}", p.message);
    assert_eq!(p.line, Some(3));

    let p = err("config.add_global_data(\"x\", || #{ f: || 1 });");
    assert_eq!(p.code, codes::CONFIG_INVALID);
    assert!(
        p.message
            .contains("add_global_data(\"x\", ...): its function returned"),
        "{}",
        p.message
    );
    assert_eq!(p.line, Some(1));
}

/// Numbers print as JavaScript prints them. Probed with Node: `"n" + 7.5`, `2 + "x"`,
/// `String(5)`, `String(0.1 + 0.2)`, `new Date(NaN).getFullYear()`, `"a-b-c".replace(/-/, "+")`, `${1e21}`,
/// `${10} per page`.
#[test]
fn numbers_print_as_javascript_prints_them() {
    let c = ok(r#"
        config.add_global_data("a", "n" + 7.5);
        config.add_global_data("b", 2.0 + "x");
        config.add_global_data("c", (5.0).to_string());
        config.add_global_data("d", (0.1 + 0.2).to_string());
        config.add_global_data("e", year(date(())));
        config.add_global_data("f", regex_replace("a-b-c", "-", "+"));
        config.add_global_data("g", `${1e21}`);
        const PER_PAGE = 10.0;
        config.add_global_data("h", `${PER_PAGE} per page`);
    "#);
    let g = |k: &str| c.global_data[k].clone();
    assert_eq!(g("a"), s("n7.5"));
    assert_eq!(g("b"), s("2x"));
    assert_eq!(g("c"), s("5"));
    assert_eq!(g("d"), s("0.30000000000000004"));
    let Value::Number(nan) = g("e") else {
        panic!("{:?}", g("e"))
    };
    assert!(nan.is_nan());
    assert_eq!(g("f"), s("a+b-c"));
    // Constants too: the optimizer would fold these with Rhai's formatting, `1e21` and `10.0`.
    assert_eq!(g("g"), s("1e+21"));
    assert_eq!(g("h"), s("10 per page"));
}

#[test]
fn set_js_data_turns_javascript_data_on_or_off() {
    use brrrty_core::JsData;
    let on = ok("let x = 1;\nconfig.set_js_data(\"node\");\n").build_options();
    assert_eq!(on.js_data, Some((JsData::Node, Some(2))));
    let off = ok("config.set_js_data(\"off\");").build_options();
    assert_eq!(off.js_data, Some((JsData::Off, Some(1))));
    // A later call wins.
    let last = ok("config.set_js_data(\"node\");\nconfig.set_js_data(\"off\");").build_options();
    assert_eq!(last.js_data, Some((JsData::Off, Some(2))));
    assert_eq!(ok("let x = 1;").build_options().js_data, None);
}

#[test]
fn set_js_data_names_the_two_values_it_takes() {
    let p = err("\nconfig.set_js_data(\"yes\");");
    assert_eq!(p.code, codes::CONFIG_JS_DATA_INVALID);
    assert!(
        p.message.contains("\"node\"") && p.message.contains("\"off\""),
        "{}",
        p.message
    );
    assert_eq!(p.line, Some(2));
    assert!(p.column.is_some());
}

/// `dir.data` is a folder inside the input folder, like includes.
#[test]
fn data_folder_is_read() {
    let c = ok(r#"#{ dir: #{ data: "./data/" } }"#);
    assert_eq!(c.dir.data.as_deref(), Some("data"));
    assert_eq!(c.build_options().data_dir(), "data");
    assert_eq!(ok("").build_options().data_dir(), "_data");
    let p = err(r#"#{ dir: #{ data: "../data" } }"#);
    assert_eq!(p.code, codes::CONFIG_UNSUPPORTED);
    assert!(p.message.contains("dir.data"), "{}", p.message);
}
