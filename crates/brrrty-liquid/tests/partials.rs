//! `include` and `render`: the scope rules and name handling pinned against
//! LiquidJS 10.30.1 (run as 11ty configures it), and Brrrty's own errors.

use std::collections::BTreeMap;
use std::sync::Mutex;

use brrrty_js::Value;
use brrrty_liquid::{Engine, EngineOptions, ErrorKind, Loader, Tz};

#[derive(Default)]
struct Files(BTreeMap<String, String>);

impl Files {
    fn new(files: &[(&str, &str)]) -> Files {
        Files(
            files
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }
}

impl Loader for Files {
    fn load(&self, name: &str) -> Option<(String, String)> {
        self.0
            .get(name)
            .map(|s| (format!("_includes/{name}"), s.clone()))
    }
    fn search_paths(&self, name: &str) -> Vec<String> {
        vec![format!("_includes/{name}"), format!("./{name}")]
    }
}

fn engine() -> Engine {
    Engine::new(EngineOptions {
        strict_filters: true,
        timezone: Tz::Utc,
    })
}

fn data() -> Value {
    let j: serde_json::Value =
        serde_json::from_str(r#"{"d":"D","n":5,"xs":[1,2],"o":{"a":1},"item":"IT","nm":"plain"}"#)
            .unwrap();
    Value::from_json(&j)
}

fn run(template: &str, files: &Files) -> Result<String, brrrty_liquid::LiquidError> {
    let e = engine();
    let t = e.parse("page", template)?;
    e.render(&t, &data(), files)
}

fn files() -> Files {
    Files::new(&[
        ("plain.liquid", "NOEXT"),
        ("sub/deep.liquid", "DEEP"),
        (
            "show.liquid",
            "[{{ v }}|{{ d }}|{{ title }}|{{ p }}|{{ item }}|{{ forloop.index }}]",
        ),
        ("p.liquid", "P:{{ p }}"),
        ("mk.liquid", "{% assign made = 1 %}{{ made }}"),
        ("frag.md", "MD"),
        ("x.", "DOT"),
        ("fn.liquid", "{{ forloop.name }}"),
    ])
}

/// Every pair is `(template, output)` from LiquidJS 10.30.1 with
/// `strictFilters` on and the files above.
#[test]
fn partials_match_liquidjs() {
    let cases: &[(&str, &str)] = &[
        ("{% include 'sub/deep' | x %}", "DEEP"),
        ("{% include 'show' with xs %}", "[|D|||IT|]"),
        ("{% include 'p' with o %}", "P:[object Object]"),
        ("{% include 'p' with item %}", "P:IT"),
        ("{% include 'show' for xs %}", "[|D|||IT|]"),
        ("{% include 'show' as z %}", "[|D|||IT|]"),
        ("{% include 'show' title: 'T', p: 1 %}", "[|D|T|1|IT|]"),
        ("{% include 'p', p: 'a' p: 'b' %}", "P:b"),
        ("{% assign v = 'V' %}{% include 'show' %}", "[V|D|||IT|]"),
        ("{% include nm %}", "NOEXT"),
        ("{% include 'frag.md' | upcase %}", "MD"),
        ("{% include 'mk' %}{{ made }}", "11"),
        ("{% render 'show' %}", "[|||||]"),
        ("{% render 'show', title: 'T' %}", "[||T|||]"),
        ("{% render 'show', a: d %}{{ a }}", "[|||||]"),
        ("{% render 'show' with 'a' %}", "[|||||]"),
        ("{% render 'show' with xs as q %}", "[|||||]"),
        ("{% render 'p' with item as p %}", "P:IT"),
        ("{% render 'p' with item %}", "P:IT"),
        ("{% render 'p' for xs as p %}", "P:1P:2"),
        ("{% render 'show' for xs %}", "[|||||1][|||||2]"),
        ("{% render 'show' for 3 as q %}", ""),
        ("{% render 'show' for o as q %}", "[|||||1]"),
        ("{% render 'show', with: 1 %}", "[|||||]"),
        ("{% render 'fn' for xs %}", "undefined-xsundefined-xs"),
        ("{% render 'fn' for xs as q %}", "q-xsq-xs"),
        ("{% render 'fn' for (1..2) as q %}", "q-(1..2)q-(1..2)"),
        ("{% render 'nope' for e as q %}", ""),
        ("{% render 'mk' %}{{ made }}", "1"),
        ("{% assign v = 'V' %}{% render 'show' %}", "[|||||]"),
        (
            "{% for i in xs %}{% render 'show' %}{% endfor %}",
            "[|||||][|||||]",
        ),
        (
            "{% for i in xs %}{% include 'show' %}{% endfor %}",
            "[|D|||IT|1][|D|||IT|2]",
        ),
    ];
    let f = files();
    let mut failures = Vec::new();
    for (t, want) in cases {
        match run(t, &f) {
            Ok(got) if got == *want => {}
            Ok(got) => failures.push(format!("{t}: want {want:?}, got {got:?}")),
            Err(e) => failures.push(format!("{t}: want {want:?}, got error {e}")),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// LiquidJS's `path.extname` rule: a name with no extension gets
/// `.liquid`. The loader receives the name with it.
#[test]
fn names_get_liquid_when_they_have_no_extension() {
    struct Spy(Mutex<Vec<String>>);
    impl Loader for Spy {
        fn load(&self, name: &str) -> Option<(String, String)> {
            self.0.lock().unwrap().push(name.to_string());
            Some((name.to_string(), String::new()))
        }
    }
    let spy = Spy(Mutex::new(Vec::new()));
    let e = engine();
    let names = [
        "plain", "a.b/c", ".hidden", "x.md", "x.", "sub/.h", "a.tar.gz", "..",
    ];
    for n in names {
        let t = e.parse("page", &format!("{{% include '{n}' %}}")).unwrap();
        e.render(&t, &data(), &spy).unwrap();
    }
    assert_eq!(
        *spy.0.lock().unwrap(),
        [
            "plain.liquid",
            "a.b/c.liquid",
            ".hidden.liquid",
            "x.md",
            "x.",
            "sub/.h.liquid",
            "a.tar.gz",
            "...liquid"
        ]
    );
}

#[test]
fn unquoted_name_that_is_not_a_string_says_to_quote_it() {
    for tag in ["include", "render"] {
        let e = run(&format!("a\n{{% {tag} part4 %}}"), &files()).unwrap_err();
        assert_eq!(e.message, "illegal file path \"undefined\"");
        assert_eq!(
            e.help.as_deref(),
            Some(&*format!(
                "{tag} names must be quoted: {{% {tag} \"name\" %}}"
            ))
        );
        assert_eq!((e.line, e.column), (2, 1));
        assert_eq!(e.kind, ErrorKind::Render);
    }
    // A variable that holds a string is fine, as in LiquidJS.
    assert_eq!(run("{% include nm %}", &files()).unwrap(), "NOEXT");
}

#[test]
fn empty_and_non_string_names_are_illegal_paths() {
    let e = run("{% include '' %}", &files()).unwrap_err();
    assert_eq!(e.message, "illegal file path \"\"");
    let e = run("{% include n %}", &files()).unwrap_err();
    assert_eq!(e.message, "illegal file path \"5\"");
}

#[test]
fn missing_file_names_what_was_tried() {
    let e = run("x\n  {% include 'nope' %}", &files()).unwrap_err();
    assert!(e.message.contains("nope.liquid"), "{}", e.message);
    assert_eq!((e.line, e.column), (2, 3));
    let help = e.help.unwrap();
    assert!(help.contains("_includes/nope.liquid"), "{help}");
    assert!(help.contains("./nope.liquid"), "{help}");
    assert!(help.contains("no extension"), "{help}");

    let e = run("{% render 'nope.md' %}", &files()).unwrap_err();
    assert!(e.message.contains("nope.md"), "{}", e.message);
    assert!(!e.help.unwrap().contains("no extension"));
}

#[test]
fn cycles_are_named() {
    let f = Files::new(&[
        ("a.liquid", "a{% include 'b' %}"),
        ("b.liquid", "b{% include 'a' %}"),
        ("me.liquid", "{% render 'me' %}"),
    ]);
    let e = run("{% include 'a' %}", &f).unwrap_err();
    assert_eq!(e.message, "include cycle: a -> b -> a");
    let e = run("{% include 'me' %}", &f).unwrap_err();
    assert_eq!(e.message, "include cycle: me -> me");
}

/// A chain of 64 partials renders. A 65th is an error, cycle or not.
#[test]
fn nesting_is_limited_to_64_levels() {
    struct Chain(usize);
    impl Loader for Chain {
        fn load(&self, name: &str) -> Option<(String, String)> {
            let n: usize = name
                .strip_prefix('d')?
                .strip_suffix(".liquid")?
                .parse()
                .ok()?;
            let body = if n < self.0 {
                format!("{{% include 'd{}' %}}", n + 1)
            } else {
                "END".to_string()
            };
            Some((name.to_string(), body))
        }
    }
    let e = engine();
    let t = e.parse("page", "{% include 'd1' %}").unwrap();
    assert_eq!(e.render(&t, &data(), &Chain(64)).unwrap(), "END");
    let err = e.render(&t, &data(), &Chain(65)).unwrap_err();
    assert!(err.message.contains("64 levels"), "{}", err.message);
}

#[test]
fn errors_inside_a_partial_name_the_partial() {
    let f = Files::new(&[("bad.liquid", "ok\n  {{ 'x' | nosuchfilter }}")]);
    let e = run("{% include 'bad' %}", &f).unwrap_err();
    assert_eq!(e.message, "undefined filter: nosuchfilter");
    assert_eq!(e.template, "_includes/bad.liquid");
    assert_eq!((e.line, e.column), (2, 3));
}
